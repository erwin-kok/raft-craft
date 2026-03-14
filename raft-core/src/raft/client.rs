use crate::{
    Raft,
    protocol::{action::Action, command::Command, log::LogEntry},
};

impl Raft {
    /// Handle a command submitted by a client.
    ///
    /// **Leader behaviour (§5.3)**
    /// Appends the command to the local log with the current term and the next
    /// available index, then returns `Action::ResetHeartbeatTimer` to trigger
    /// an immediate `AppendEntries` broadcast rather than waiting for the next
    /// scheduled heartbeat tick. The broadcast itself is handled by
    /// `handle_heartbeat_timeout`, which reads `next_index` per peer and
    /// sends the right slice of entries to each one.
    ///
    /// In a single-node cluster there are no peers to replicate to, so the
    /// leader is the entire quorum and the entry is committed immediately by
    /// calling `try_advance_commit_index` inline.
    ///
    /// **Non-leader behaviour**
    /// Returns `Action::NotLeader` so the caller can redirect or queue the
    /// request. The action carries the node's best knowledge of the current
    /// leader id, which may be `None` if no leader has been observed yet.
    ///
    /// Note: `Action::NotLeader(Option<NodeId>)` must be present in the
    /// `Action` enum. Add it if it is not already there.
    pub(crate) fn handle_client_request(&mut self, command: Command) -> Vec<Action> {
        if self.role != crate::Role::Leader {
            return vec![Action::NotLeader(self.known_leader)];
        }

        let index = self.last_log_index() + 1;
        let term = self.persistent.current_term;

        self.persistent.log.push(LogEntry {
            index,
            term,
            command,
        });

        // In a single-node cluster the leader is the entire quorum, so the
        // entry is committed the moment it is appended. No peers means no
        // AppendEntries responses will ever arrive, so try_advance_commit_index
        // would never be called otherwise.
        let mut actions = vec![Action::ResetHeartbeatTimer];
        if self.peers.is_empty() {
            actions.extend(self.try_advance_commit_index());
        }

        actions
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        LeaderState, Raft,
        protocol::{action::Action, command::Command, log::LogEntry, types::NodeId},
    };

    // ── non-leader guards ─────────────────────────────────────────────────────

    #[test]
    fn follower_returns_not_leader() {
        let mut raft = new_raft(1, &[2, 3]);
        assert_eq!(raft.role, crate::Role::Follower);

        let actions = raft.handle_client_request(Command::default());

        assert!(
            matches!(actions[..], [Action::NotLeader(_)]),
            "follower must return NotLeader"
        );
    }

    #[test]
    fn candidate_returns_not_leader() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = crate::Role::Candidate;

        let actions = raft.handle_client_request(Command::default());

        assert!(matches!(actions[..], [Action::NotLeader(_)]));
    }

    #[test]
    fn not_leader_carries_known_leader_id() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.known_leader = Some(2);

        let actions = raft.handle_client_request(Command::default());

        assert!(matches!(actions[..], [Action::NotLeader(Some(2))]));
    }

    #[test]
    fn not_leader_carries_none_when_leader_unknown() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.known_leader = None;

        let actions = raft.handle_client_request(Command::default());

        assert!(matches!(actions[..], [Action::NotLeader(None)]));
    }

    #[test]
    fn follower_does_not_append_to_log() {
        let mut raft = new_raft(1, &[2, 3]);

        raft.handle_client_request(Command::default());

        assert!(raft.persistent.log.is_empty());
    }

    // ── leader: log append ────────────────────────────────────────────────────

    #[test]
    fn leader_appends_entry_to_log() {
        let mut raft = make_leader(1, &[2, 3]);

        raft.handle_client_request(Command::default());

        assert_eq!(raft.persistent.log.len(), 1);
    }

    #[test]
    fn appended_entry_has_current_term() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 4;

        raft.handle_client_request(Command::default());

        assert_eq!(raft.persistent.log[0].term, 4);
    }

    #[test]
    fn appended_entry_has_next_available_index_on_empty_log() {
        let mut raft = make_leader(1, &[2, 3]);

        raft.handle_client_request(Command::default());

        assert_eq!(raft.persistent.log[0].index, 1);
    }

    #[test]
    fn appended_entry_follows_last_log_index() {
        let mut raft = make_leader(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);

        raft.handle_client_request(Command::default());

        assert_eq!(raft.persistent.log.last().unwrap().index, 3);
    }

    #[test]
    fn appended_entry_carries_the_submitted_command() {
        let mut raft = make_leader(1, &[2, 3]);
        let cmd = Command::new(42);

        raft.handle_client_request(cmd.clone());

        assert_eq!(raft.persistent.log[0].command, cmd);
    }

    #[test]
    fn multiple_requests_produce_sequential_indices() {
        let mut raft = make_leader(1, &[2, 3]);

        for _ in 0..5 {
            raft.handle_client_request(Command::default());
        }

        let indices: Vec<u64> = raft.persistent.log.iter().map(|e| e.index).collect();
        assert_eq!(indices, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn multiple_requests_all_use_current_term() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 7;

        for _ in 0..3 {
            raft.handle_client_request(Command::default());
        }

        assert!(raft.persistent.log.iter().all(|e| e.term == 7));
    }

    // ── leader: actions ───────────────────────────────────────────────────────

    #[test]
    fn leader_returns_reset_heartbeat_timer() {
        let mut raft = make_leader(1, &[2, 3]);

        let actions = raft.handle_client_request(Command::default());

        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::ResetHeartbeatTimer)),
            "leader must include ResetHeartbeatTimer"
        );
    }

    #[test]
    fn multi_node_leader_returns_exactly_one_action() {
        let mut raft = make_leader(1, &[2, 3]);

        let actions = raft.handle_client_request(Command::default());

        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn multi_node_leader_does_not_commit_entry_immediately() {
        let mut raft = make_leader(1, &[2, 3]);

        raft.handle_client_request(Command::default());

        assert_eq!(
            raft.volatile.commit_index, 0,
            "commit only happens after quorum"
        );
    }

    #[test]
    fn multi_node_leader_does_not_apply_entry_immediately() {
        let mut raft = make_leader(1, &[2, 3]);

        raft.handle_client_request(Command::default());

        assert_eq!(raft.volatile.last_applied, 0);
    }

    // ── leader: next_index unchanged (send is deferred to heartbeat) ──────────

    #[test]
    fn leader_does_not_change_next_index_on_append() {
        let mut raft = make_leader(1, &[2, 3]);
        let next_before: Vec<u64> = raft.leader_state.as_ref().unwrap().next_index.clone();

        raft.handle_client_request(Command::default());

        let next_after = &raft.leader_state.as_ref().unwrap().next_index;
        assert_eq!(
            &next_before, next_after,
            "next_index is advanced by handle_append_entries_response, not here"
        );
    }

    // ── single-node cluster: immediate commit ─────────────────────────────────

    #[test]
    fn single_node_leader_commits_entry_immediately() {
        // In a single-node cluster the leader is the entire quorum. There are
        // no AppendEntries responses to wait for, so handle_client_request
        // must call try_advance_commit_index itself immediately after the append.
        let mut raft = make_leader(1, &[]);

        let actions = raft.handle_client_request(Command::default());

        assert_eq!(raft.volatile.commit_index, 1);
        assert_eq!(raft.volatile.last_applied, 1);
        assert!(
            actions.iter().any(|a| matches!(a, Action::ApplyCommand(_))),
            "ApplyCommand must be returned so the state machine is notified"
        );
    }

    #[test]
    fn single_node_multiple_requests_all_committed_immediately() {
        let mut raft = make_leader(1, &[]);

        for i in 1..=3u64 {
            raft.handle_client_request(Command::default());
            assert_eq!(raft.volatile.commit_index, i);
            assert_eq!(raft.volatile.last_applied, i);
        }
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn new_raft(id: NodeId, peers: &[NodeId]) -> Raft {
        Raft::new(id, peers.to_vec())
    }

    fn make_leader(id: NodeId, peers: &[NodeId]) -> Raft {
        let mut raft = new_raft(id, peers);
        raft.role = crate::Role::Leader;
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
}
