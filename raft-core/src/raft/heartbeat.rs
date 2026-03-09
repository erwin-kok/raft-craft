use crate::{
    Raft, Role,
    protocol::{action::Action, message::AppendEntries, message::Message},
};

impl Raft {
    /// Called when the heartbeat timer fires. Only leaders act on this.
    ///
    /// Sends an `AppendEntries` message to every peer. When there are no new
    /// log entries to replicate the message acts as a heartbeat (empty
    /// `entries`). When the leader has entries the peer has not yet
    /// acknowledged, those entries are included.
    ///
    /// Returns `[Action::ResetHeartbeatTimer]` plus one `Action::Send` per
    /// peer, or an empty `Vec` if the node is not the leader.
    pub(crate) fn handle_heartbeat_timeout(&mut self) -> Vec<Action> {
        if self.role != Role::Leader {
            return vec![];
        }

        let mut actions = vec![Action::ResetHeartbeatTimer];

        // Build one AppendEntries per peer.
        // We iterate by index so we can look up next_index / match_index from
        // leader_state without a borrow conflict on `self`.
        let peer_count = self.peers.len();
        for i in 0..peer_count {
            let action = self.append_entries_for_peer(i);
            actions.push(action);
        }

        actions
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Builds the `AppendEntries` message and wraps it in an `Action::Send`
    /// for the peer at position `peer_index` in `self.peers`.
    fn append_entries_for_peer(&self, peer_index: usize) -> Action {
        let peer_id = self.peers[peer_index];

        // `next_index` is the next log index we want to send to this peer.
        // Everything from `next_index` onwards is included in `entries`.
        let next_index = self
            .leader_state
            .as_ref()
            .map(|ls| ls.next_index[peer_index])
            .unwrap_or(1);

        // The entry immediately before `next_index` is the "prev" entry that
        // the follower uses to validate log consistency (§5.3).
        let (prev_log_index, prev_log_term) = self.prev_log_info(next_index);

        // Collect all entries starting at next_index.
        // Log entries are stored with their index embedded, so we find the
        // position by searching for the first entry whose index >= next_index.
        let entries = self
            .persistent
            .log
            .iter()
            .filter(|e| e.index >= next_index)
            .cloned()
            .collect();

        let msg = AppendEntries {
            term: self.persistent.current_term,
            leader_id: self.id,
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit: self.volatile.commit_index,
        };

        Action::Send(peer_id, Message::AppendEntries(msg))
    }

    /// Returns `(prev_log_index, prev_log_term)` for the entry immediately
    /// before `next_index`. Returns `(0, 0)` when `next_index` is 1 (i.e.
    /// the peer needs everything from the start of the log).
    fn prev_log_info(&self, next_index: u64) -> (u64, u64) {
        if next_index <= 1 {
            return (0, 0);
        }
        let prev_index = next_index - 1;
        let prev_term = self
            .persistent
            .log
            .iter()
            .find(|e| e.index == prev_index)
            .map(|e| e.term)
            .unwrap_or(0);
        (prev_index, prev_term)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        LeaderState, Raft, Role,
        protocol::{
            action::Action,
            command::Command,
            log::LogEntry,
            message::{AppendEntries, Message},
            types::NodeId,
        },
    };

    // ── role guard ────────────────────────────────────────────────────────────

    #[test]
    fn follower_does_nothing() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Follower;

        assert!(raft.handle_heartbeat_timeout().is_empty());
    }

    #[test]
    fn candidate_does_nothing() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Candidate;

        assert!(raft.handle_heartbeat_timeout().is_empty());
    }

    // ── timer reset ───────────────────────────────────────────────────────────

    #[test]
    fn leader_resets_heartbeat_timer() {
        let mut raft = make_leader(1, &[2, 3]);

        let actions = raft.handle_heartbeat_timeout();

        let count = actions
            .iter()
            .filter(|a| matches!(a, Action::ResetHeartbeatTimer))
            .count();
        assert_eq!(count, 1, "exactly one ResetHeartbeatTimer expected");
    }

    // ── message count and recipients ─────────────────────────────────────────

    #[test]
    fn sends_one_message_per_peer() {
        let mut raft = make_leader(1, &[2, 3, 4]);

        let actions = raft.handle_heartbeat_timeout();

        let sends: Vec<NodeId> = append_entry_targets(&actions);
        assert_eq!(sends.len(), 3);
    }

    #[test]
    fn messages_go_to_correct_peers() {
        let mut raft = make_leader(1, &[2, 3, 4]);

        let actions = raft.handle_heartbeat_timeout();

        let targets = append_entry_targets(&actions);
        assert!(targets.contains(&2));
        assert!(targets.contains(&3));
        assert!(targets.contains(&4));
        assert!(!targets.contains(&1), "leader must not send to itself");
    }

    #[test]
    fn single_peer_cluster_sends_one_message() {
        let mut raft = make_leader(1, &[2]);

        let actions = raft.handle_heartbeat_timeout();

        assert_eq!(append_entry_targets(&actions).len(), 1);
    }

    // ── empty log / pure heartbeat ────────────────────────────────────────────

    #[test]
    fn heartbeat_with_empty_log_sends_zero_prev_and_empty_entries() {
        let mut raft = make_leader(1, &[2, 3]);

        let actions = raft.handle_heartbeat_timeout();

        for msg in append_entries(&actions) {
            assert_eq!(msg.prev_log_index, 0);
            assert_eq!(msg.prev_log_term, 0);
            assert!(msg.entries.is_empty());
        }
    }

    // ── term and leader_id fields ─────────────────────────────────────────────

    #[test]
    fn messages_carry_current_term_and_leader_id() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 7;

        let actions = raft.handle_heartbeat_timeout();

        for msg in append_entries(&actions) {
            assert_eq!(msg.term, 7);
            assert_eq!(msg.leader_id, 1);
        }
    }

    // ── leader_commit ─────────────────────────────────────────────────────────

    #[test]
    fn messages_carry_leader_commit_index() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.volatile.commit_index = 5;

        let actions = raft.handle_heartbeat_timeout();

        for msg in append_entries(&actions) {
            assert_eq!(msg.leader_commit, 5);
        }
    }

    // ── log replication ───────────────────────────────────────────────────────

    #[test]
    fn includes_entries_peer_has_not_yet_seen() {
        let mut raft = make_leader(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);
        push_entry(&mut raft, 3, 1);

        // Peer 2 (index 0 in peers) is up to date (next_index = 4).
        // Peer 3 (index 1 in peers) is behind (next_index = 2).
        raft.leader_state.as_mut().unwrap().next_index[0] = 4;
        raft.leader_state.as_mut().unwrap().next_index[1] = 2;

        let actions = raft.handle_heartbeat_timeout();

        let msg_for_2 = find_append_entries(&actions, 2);
        assert!(msg_for_2.entries.is_empty(), "peer 2 is up to date");

        let msg_for_3 = find_append_entries(&actions, 3);
        assert_eq!(msg_for_3.entries.len(), 2);
        assert_eq!(msg_for_3.entries[0].index, 2);
        assert_eq!(msg_for_3.entries[1].index, 3);
    }

    #[test]
    fn prev_log_fields_reflect_entry_before_next_index() {
        let mut raft = make_leader(1, &[2]);
        push_entry(&mut raft, 1, 3);
        push_entry(&mut raft, 2, 5);
        push_entry(&mut raft, 3, 5);

        // Peer 2 needs entries starting at index 3, so prev = (2, 5).
        raft.leader_state.as_mut().unwrap().next_index[0] = 3;

        let actions = raft.handle_heartbeat_timeout();

        let msg = find_append_entries(&actions, 2);
        assert_eq!(msg.prev_log_index, 2);
        assert_eq!(msg.prev_log_term, 5);
        assert_eq!(msg.entries.len(), 1);
        assert_eq!(msg.entries[0].index, 3);
    }

    #[test]
    fn peer_needs_full_log_from_beginning() {
        let mut raft = make_leader(1, &[2]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 2);

        // Peer 2 has nothing yet (next_index = 1).
        raft.leader_state.as_mut().unwrap().next_index[0] = 1;

        let actions = raft.handle_heartbeat_timeout();

        let msg = find_append_entries(&actions, 2);
        assert_eq!(msg.prev_log_index, 0);
        assert_eq!(msg.prev_log_term, 0);
        assert_eq!(msg.entries.len(), 2);
    }

    #[test]
    fn all_peers_up_to_date_sends_empty_entries() {
        let mut raft = make_leader(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);

        // Both peers are fully caught up.
        raft.leader_state.as_mut().unwrap().next_index[0] = 3;
        raft.leader_state.as_mut().unwrap().next_index[1] = 3;

        let actions = raft.handle_heartbeat_timeout();

        for msg in append_entries(&actions) {
            assert!(msg.entries.is_empty());
            assert_eq!(msg.prev_log_index, 2);
            assert_eq!(msg.prev_log_term, 1);
        }
    }

    #[test]
    fn peers_at_different_offsets_receive_different_entries() {
        let mut raft = make_leader(1, &[2, 3, 4]);
        for i in 1..=5u64 {
            push_entry(&mut raft, i, 1);
        }

        // next_index: peer2=1 (needs all), peer3=4 (needs last 2), peer4=6 (up to date)
        raft.leader_state.as_mut().unwrap().next_index[0] = 1;
        raft.leader_state.as_mut().unwrap().next_index[1] = 4;
        raft.leader_state.as_mut().unwrap().next_index[2] = 6;

        let actions = raft.handle_heartbeat_timeout();

        assert_eq!(find_append_entries(&actions, 2).entries.len(), 5);
        assert_eq!(find_append_entries(&actions, 3).entries.len(), 2);
        assert!(find_append_entries(&actions, 4).entries.is_empty());
    }

    // ── idempotency ───────────────────────────────────────────────────────────

    #[test]
    fn multiple_heartbeats_produce_same_messages() {
        let mut raft = make_leader(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);

        let a1 = raft.handle_heartbeat_timeout();
        let a2 = raft.handle_heartbeat_timeout();

        let entries1: Vec<_> = append_entries(&a1)
            .into_iter()
            .map(|m| m.entries.len())
            .collect();
        let entries2: Vec<_> = append_entries(&a2)
            .into_iter()
            .map(|m| m.entries.len())
            .collect();
        assert_eq!(
            entries1, entries2,
            "heartbeat is idempotent until next_index changes"
        );
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn new_raft(id: NodeId, peers: &[NodeId]) -> Raft {
        Raft::new(id, peers.to_vec())
    }

    /// Create a node that is already the leader with fresh leader_state.
    fn make_leader(id: NodeId, peers: &[NodeId]) -> Raft {
        let mut raft = new_raft(id, peers);
        raft.role = Role::Leader;
        raft.leader_state = Some(LeaderState {
            next_index: vec![1; peers.len()],
            match_index: vec![0; peers.len()],
        });
        raft
    }

    fn push_entry(raft: &mut Raft, index: u64, term: u64) {
        raft.persistent.log.push(LogEntry {
            index,
            term,
            command: Command::default(),
        });
    }

    fn append_entry_targets(actions: &[Action]) -> Vec<NodeId> {
        actions
            .iter()
            .filter_map(|a| {
                if let Action::Send(id, Message::AppendEntries(_)) = a {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect()
    }

    fn append_entries(actions: &[Action]) -> Vec<&AppendEntries> {
        actions
            .iter()
            .filter_map(|a| {
                if let Action::Send(_, Message::AppendEntries(m)) = a {
                    Some(m)
                } else {
                    None
                }
            })
            .collect()
    }

    fn find_append_entries(actions: &[Action], peer: NodeId) -> &AppendEntries {
        actions
            .iter()
            .find_map(|a| {
                if let Action::Send(id, Message::AppendEntries(m)) = a {
                    if *id == peer { Some(m) } else { None }
                } else {
                    None
                }
            })
            .unwrap_or_else(|| panic!("no AppendEntries found for peer {peer}"))
    }
}
