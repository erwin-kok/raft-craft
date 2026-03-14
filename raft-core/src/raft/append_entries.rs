use crate::{
    Raft,
    protocol::{
        action::Action,
        message::{AppendEntries, AppendEntriesResponse, Message},
        types::NodeId,
    },
};

impl Raft {
    /// Follower / candidate side: process an incoming `AppendEntries` RPC.
    ///
    /// Implements Raft §5.1–§5.3 receiver logic:
    /// 1. Always step down when `msg.term > current_term`.
    /// 2. Reject if `msg.term < current_term` (stale leader).
    /// 3. Reject if our log does not contain `prev_log_index` at `prev_log_term`
    ///    (consistency check).
    /// 4. Delete any conflicting suffix, then append new entries.
    /// 5. Advance `commit_index` to `min(leader_commit, last_new_entry_index)`.
    /// 6. Reset the election timer and reply success.
    pub(crate) fn handle_append_entries(&mut self, msg: AppendEntries) -> Vec<Action> {
        // Step down on higher term before anything else (§5.1).
        if msg.term > self.persistent.current_term {
            self.step_down(msg.term);
        }

        // Reject stale leader.
        if msg.term < self.persistent.current_term {
            return vec![self.append_entries_response(msg.leader_id, false)];
        }

        // At this point msg.term == current_term. A candidate receiving a valid
        // AppendEntries from the current term's leader must revert to follower.
        if self.role == crate::Role::Candidate {
            self.role = crate::Role::Follower;
            self.candidate_state = None;
        }

        // Consistency check: our log must contain an entry at prev_log_index
        // whose term matches prev_log_term (§5.3).
        if !self.log_matches(msg.prev_log_index, msg.prev_log_term) {
            return vec![self.append_entries_response(msg.leader_id, false)];
        }

        // We know who the leader is.
        self.known_leader = Some(msg.leader_id);

        // Reconcile incoming entries with the existing log.
        // Walk each new entry; if it conflicts with what we have (same index,
        // different term), truncate from that point and append the rest.
        for (i, new_entry) in msg.entries.iter().enumerate() {
            match self
                .persistent
                .log
                .iter()
                .position(|e| e.index == new_entry.index)
            {
                Some(pos) if self.persistent.log[pos].term != new_entry.term => {
                    // Conflict: drop this entry and everything after it, then
                    // append remaining entries from the message.
                    self.persistent.log.truncate(pos);
                    self.persistent.log.extend_from_slice(&msg.entries[i..]);
                    break;
                }
                None => {
                    // Entry is beyond our log — append this and all remaining.
                    self.persistent.log.extend_from_slice(&msg.entries[i..]);
                    break;
                }
                Some(_) => {
                    // Entry already present and terms match — skip.
                }
            }
        }

        // Advance commit index (§5.3).
        if msg.leader_commit > self.volatile.commit_index {
            let last_new_index = msg.entries.last().map_or(msg.prev_log_index, |e| e.index);
            self.volatile.commit_index = msg.leader_commit.min(last_new_index);
        }

        // Apply any newly committed entries.
        let apply_actions = self.apply_committed_entries();

        let mut actions = vec![
            Action::ResetElectionTimer,
            self.append_entries_response(msg.leader_id, true),
        ];
        actions.extend(apply_actions);
        actions
    }

    /// Leader side: process a peer's reply to `AppendEntries`.
    ///
    /// 1. Always step down on a higher term (§5.1).
    /// 2. Ignore stale / out-of-role / unknown-peer responses.
    /// 3. On failure: decrement `next_index` so the next heartbeat retries
    ///    with an earlier entry (§5.3).
    /// 4. On success: advance `next_index` and `match_index` for the peer,
    ///    then check whether a new majority `commit_index` is reachable.
    pub(crate) fn handle_append_entries_response(
        &mut self,
        msg: AppendEntriesResponse,
    ) -> Vec<Action> {
        // Step down on higher term before anything else (§5.1).
        if msg.term > self.persistent.current_term {
            self.step_down(msg.term);
            return vec![];
        }

        // Ignore stale responses.
        if msg.term < self.persistent.current_term {
            return vec![];
        }

        // Only leaders process these.
        if self.role != crate::Role::Leader {
            return vec![];
        }

        // Find the peer's index in self.peers.
        let Some(peer_index) = self.peers.iter().position(|&id| id == msg.from) else {
            return vec![];
        };

        let ls = match self.leader_state.as_mut() {
            Some(ls) => ls,
            None => return vec![],
        };

        if !msg.success {
            // Log inconsistency: back up next_index by one (floor at 1).
            ls.next_index[peer_index] = ls.next_index[peer_index].saturating_sub(1).max(1);
            return vec![];
        }

        // Success: the peer has appended everything up to msg.match_index.
        // Only advance — never go backwards (duplicate / reordered responses).
        if msg.match_index > ls.match_index[peer_index] {
            ls.match_index[peer_index] = msg.match_index;
            ls.next_index[peer_index] = msg.match_index + 1;
        }

        // Check whether we can advance commit_index.
        // Find the highest index N such that:
        //   • N > current commit_index
        //   • log[N].term == current_term  (§5.4 — only commit own-term entries)
        //   • a quorum of match_index values >= N  (self always counts)
        self.try_advance_commit_index()
    }

    /// Try to advance `commit_index` to the highest N where:
    /// - N > current `commit_index`
    /// - `log[N].term == current_term`
    /// - a quorum of nodes (self + peers) have `match_index >= N`
    ///
    /// Returns `ApplyCommand` actions for all newly committed entries.
    pub(crate) fn try_advance_commit_index(&mut self) -> Vec<Action> {
        let current_term = self.persistent.current_term;
        let current_commit = self.volatile.commit_index;
        let quorum = self.quorum();

        // Collect match indices: self always has its full log.
        // self's match index is effectively last_log_index().
        let peer_matches: Vec<u64> = self
            .leader_state
            .as_ref()
            .map(|ls| ls.match_index.clone())
            .unwrap_or_default();

        // Find the highest index that satisfies all three conditions.
        // Iterate from the top down so we commit as much as possible at once.
        let mut new_commit = current_commit;
        for entry in self.persistent.log.iter().rev() {
            if entry.index <= current_commit {
                break; // everything below is already committed
            }
            if entry.term != current_term {
                continue; // §5.4: only commit entries from the current term
            }
            // Count how many nodes have this entry.
            let replicated = 1 /* self */
                + peer_matches.iter().filter(|&&m| m >= entry.index).count();
            if replicated >= quorum {
                new_commit = entry.index;
                break; // highest qualifying index found
            }
        }

        if new_commit > self.volatile.commit_index {
            self.volatile.commit_index = new_commit;
        }

        self.apply_committed_entries()
    }

    // ── private helpers ───────────────────────────────────────────────────────

    /// Returns true if our log contains an entry at `index` with the given
    /// `term`, or if `index == 0` (the empty-log base case).
    fn log_matches(&self, index: u64, term: u64) -> bool {
        if index == 0 {
            return true;
        }
        self.persistent
            .log
            .iter()
            .any(|e| e.index == index && e.term == term)
    }

    /// Emit `Action::ApplyCommand` for every entry in
    /// `(last_applied, commit_index]` and advance `last_applied`.
    pub(crate) fn apply_committed_entries(&mut self) -> Vec<Action> {
        let mut actions = vec![];
        while self.volatile.last_applied < self.volatile.commit_index {
            self.volatile.last_applied += 1;
            let target = self.volatile.last_applied;
            if let Some(entry) = self.persistent.log.iter().find(|e| e.index == target) {
                actions.push(Action::ApplyCommand(entry.command.clone()));
            }
        }
        actions
    }

    fn append_entries_response(&self, leader_id: NodeId, success: bool) -> Action {
        Action::Send(
            leader_id,
            Message::AppendEntriesResponse(AppendEntriesResponse {
                term: self.persistent.current_term,
                success,
                from: self.id,
                match_index: if success { self.last_log_index() } else { 0 },
            }),
        )
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::{
        LeaderState, Raft,
        protocol::{
            action::Action,
            command::Command,
            log::LogEntry,
            message::{AppendEntries, AppendEntriesResponse, Message},
            types::NodeId,
        },
    };

    // ════════════════════════════════════════════════════════════════════════
    // handle_append_entries
    // ════════════════════════════════════════════════════════════════════════

    // ── term checks ───────────────────────────────────────────────────────────

    #[test]
    fn reject_stale_leader_term() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.current_term = 5;

        let actions = raft.handle_append_entries(create_append_entries(4, 1, 0, 0, vec![], 0));

        assert!(!response_success(&actions));
        assert_eq!(response_term(&actions), 5);
        assert!(!has_election_timer_reset(&actions));
    }

    #[test]
    fn higher_term_causes_step_down_then_accepts() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = crate::Role::Leader;
        raft.persistent.current_term = 3;

        let actions = raft.handle_append_entries(create_append_entries(5, 2, 0, 0, vec![], 0));

        assert_eq!(raft.persistent.current_term, 5);
        assert_eq!(raft.role, crate::Role::Follower);
        assert!(response_success(&actions));
    }

    #[test]
    fn candidate_steps_down_on_same_term_append_entries() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = crate::Role::Candidate;
        raft.persistent.current_term = 3;

        let actions = raft.handle_append_entries(create_append_entries(3, 2, 0, 0, vec![], 0));

        assert_eq!(raft.role, crate::Role::Follower);
        assert!(raft.candidate_state.is_none());
        assert!(response_success(&actions));
    }

    // ── consistency check ─────────────────────────────────────────────────────

    #[test]
    fn reject_if_prev_log_entry_missing() {
        let mut raft = new_raft(1, &[2, 3]);
        // Log is empty but leader claims prev_log_index = 5.
        let actions = raft.handle_append_entries(create_append_entries(1, 2, 5, 1, vec![], 0));

        assert!(!response_success(&actions));
        assert!(!has_election_timer_reset(&actions));
    }

    #[test]
    fn reject_if_prev_log_term_mismatch() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 2); // index=1, term=2

        // Leader says prev entry at index 1 has term 3 — conflict.
        let actions = raft.handle_append_entries(create_append_entries(4, 2, 1, 3, vec![], 0));

        assert!(!response_success(&actions));
    }

    #[test]
    fn accept_if_prev_log_matches() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 2);

        let actions = raft.handle_append_entries(create_append_entries(3, 2, 1, 2, vec![], 0));

        assert!(response_success(&actions));
        assert!(has_election_timer_reset(&actions));
    }

    #[test]
    fn accept_with_empty_log_and_zero_prev() {
        let mut raft = new_raft(1, &[2, 3]);

        let actions = raft.handle_append_entries(create_append_entries(1, 2, 0, 0, vec![], 0));

        assert!(response_success(&actions));
        assert!(has_election_timer_reset(&actions));
    }

    // ── log reconciliation ────────────────────────────────────────────────────

    #[test]
    fn appends_new_entries_to_empty_log() {
        let mut raft = new_raft(1, &[2, 3]);

        let entries = vec![create_log_entry(1, 1), create_log_entry(2, 1)];
        raft.handle_append_entries(create_append_entries(1, 2, 0, 0, entries, 0));

        assert_eq!(raft.persistent.log.len(), 2);
        assert_eq!(raft.persistent.log[0].index, 1);
        assert_eq!(raft.persistent.log[1].index, 2);
    }

    #[test]
    fn appends_entries_after_existing_log() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);

        let entries = vec![create_log_entry(3, 1), create_log_entry(4, 1)];
        raft.handle_append_entries(create_append_entries(1, 2, 2, 1, entries, 0));

        assert_eq!(raft.persistent.log.len(), 4);
    }

    #[test]
    fn skips_entries_already_in_log() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);

        // Leader resends entries 1 and 2 which we already have.
        let entries = vec![create_log_entry(1, 1), create_log_entry(2, 1)];
        raft.handle_append_entries(create_append_entries(1, 2, 0, 0, entries, 0));

        // No duplication.
        assert_eq!(raft.persistent.log.len(), 2);
    }

    #[test]
    fn truncates_conflicting_suffix_and_appends() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);
        push_entry(&mut raft, 3, 1); // stale entry, will be overwritten

        // Leader sends index 3 with term 2 — conflicts with our term 1.
        let entries = vec![create_log_entry(3, 2), create_log_entry(4, 2)];
        raft.handle_append_entries(create_append_entries(2, 2, 2, 1, entries, 0));

        assert_eq!(raft.persistent.log.len(), 4);
        assert_eq!(raft.persistent.log[2].term, 2); // conflict resolved
        assert_eq!(raft.persistent.log[3].index, 4);
    }

    #[test]
    fn truncation_removes_all_entries_after_conflict_point() {
        let mut raft = new_raft(1, &[2, 3]);
        for i in 1..=5u64 {
            push_entry(&mut raft, i, 1);
        }

        // Conflict at index 2; leader only sends index 2 with term 2.
        let entries = vec![create_log_entry(2, 2)];
        raft.handle_append_entries(create_append_entries(2, 2, 1, 1, entries, 0));

        // Entries 3, 4, 5 must be gone.
        assert_eq!(raft.persistent.log.len(), 2);
        assert_eq!(raft.persistent.log[1].term, 2);
    }

    // ── commit index advancement ──────────────────────────────────────────────

    #[test]
    fn commit_index_advances_to_leader_commit_when_log_is_longer() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);
        push_entry(&mut raft, 3, 1);

        // Leader commit is 2, our log goes up to 3.
        raft.handle_append_entries(create_append_entries(1, 2, 3, 1, vec![], 2));

        assert_eq!(raft.volatile.commit_index, 2);
    }

    #[test]
    fn commit_index_capped_at_last_new_entry() {
        let mut raft = new_raft(1, &[2, 3]);

        // Leader commit is 5 but we only receive entries up to index 2.
        let entries = vec![create_log_entry(1, 1), create_log_entry(2, 1)];
        raft.handle_append_entries(create_append_entries(1, 2, 0, 0, entries, 5));

        assert_eq!(raft.volatile.commit_index, 2);
    }

    #[test]
    fn commit_index_does_not_go_backwards() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);
        raft.volatile.commit_index = 2;

        // Stale heartbeat with leader_commit = 1.
        raft.handle_append_entries(create_append_entries(1, 2, 2, 1, vec![], 1));

        assert_eq!(raft.volatile.commit_index, 2);
    }

    #[test]
    fn commit_index_not_advanced_when_leader_commit_is_zero() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);

        raft.handle_append_entries(create_append_entries(
            1,
            2,
            0,
            0,
            vec![create_log_entry(1, 1)],
            0,
        ));

        assert_eq!(raft.volatile.commit_index, 0);
    }

    // ── apply actions ─────────────────────────────────────────────────────────

    #[test]
    fn apply_actions_emitted_for_newly_committed_entries() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);

        let actions = raft.handle_append_entries(create_append_entries(1, 2, 2, 1, vec![], 2));

        let apply_count = actions
            .iter()
            .filter(|a| matches!(a, Action::ApplyCommand(_)))
            .count();
        assert_eq!(apply_count, 2);
        assert_eq!(raft.volatile.last_applied, 2);
    }

    #[test]
    fn no_apply_actions_when_nothing_newly_committed() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 1, 1);
        raft.volatile.commit_index = 1;
        raft.volatile.last_applied = 1;

        let actions = raft.handle_append_entries(create_append_entries(1, 2, 1, 1, vec![], 1));

        assert!(!actions.iter().any(|a| matches!(a, Action::ApplyCommand(_))));
    }

    // ── election timer ────────────────────────────────────────────────────────

    #[test]
    fn election_timer_reset_on_success() {
        let mut raft = new_raft(1, &[2, 3]);

        let actions = raft.handle_append_entries(create_append_entries(1, 2, 0, 0, vec![], 0));

        assert!(has_election_timer_reset(&actions));
    }

    #[test]
    fn no_election_timer_reset_on_rejection() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.current_term = 5;

        // Stale term → rejected.
        let actions = raft.handle_append_entries(create_append_entries(3, 2, 0, 0, vec![], 0));

        assert!(!has_election_timer_reset(&actions));
    }

    // ════════════════════════════════════════════════════════════════════════
    // handle_append_entries_response
    // ════════════════════════════════════════════════════════════════════════

    // ── term / role guards ────────────────────────────────────────────────────

    #[test]
    fn step_down_on_higher_term_response() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 4;

        let actions =
            raft.handle_append_entries_response(create_append_entries_response(6, 2, true, 3));

        assert_eq!(raft.role, crate::Role::Follower);
        assert_eq!(raft.persistent.current_term, 6);
        assert!(actions.is_empty());
    }

    #[test]
    fn ignore_stale_response() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 5;

        let actions =
            raft.handle_append_entries_response(create_append_entries_response(4, 2, true, 3));

        assert!(actions.is_empty());
        // State must be unchanged.
        assert_eq!(raft.role, crate::Role::Leader);
    }

    #[test]
    fn ignore_if_not_leader() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = crate::Role::Follower;

        let actions =
            raft.handle_append_entries_response(create_append_entries_response(1, 2, true, 1));

        assert!(actions.is_empty());
    }

    #[test]
    fn ignore_response_from_unknown_peer() {
        let mut raft = make_leader(1, &[2, 3]);

        let actions =
            raft.handle_append_entries_response(create_append_entries_response(0, 99, true, 1));

        assert!(actions.is_empty());
    }

    // ── failure handling ──────────────────────────────────────────────────────

    #[test]
    fn failure_decrements_next_index() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.leader_state.as_mut().unwrap().next_index[0] = 5; // peer 2

        raft.handle_append_entries_response(create_append_entries_response(0, 2, false, 0));

        assert_eq!(raft.leader_state.as_ref().unwrap().next_index[0], 4);
    }

    #[test]
    fn failure_does_not_decrement_next_index_below_one() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.leader_state.as_mut().unwrap().next_index[0] = 1;

        raft.handle_append_entries_response(create_append_entries_response(0, 2, false, 0));

        assert_eq!(raft.leader_state.as_ref().unwrap().next_index[0], 1);
    }

    #[test]
    fn failure_does_not_advance_match_index() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.leader_state.as_mut().unwrap().match_index[0] = 0;

        raft.handle_append_entries_response(create_append_entries_response(0, 2, false, 0));

        assert_eq!(raft.leader_state.as_ref().unwrap().match_index[0], 0);
    }

    // ── success: index advancement ────────────────────────────────────────────

    #[test]
    fn success_advances_next_and_match_index() {
        let mut raft = make_leader(1, &[2, 3]);

        raft.handle_append_entries_response(create_append_entries_response(0, 2, true, 4));

        let ls = raft.leader_state.as_ref().unwrap();
        assert_eq!(ls.match_index[0], 4);
        assert_eq!(ls.next_index[0], 5);
    }

    #[test]
    fn success_does_not_go_backwards_on_duplicate_response() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.leader_state.as_mut().unwrap().match_index[0] = 5;
        raft.leader_state.as_mut().unwrap().next_index[0] = 6;

        // Stale / duplicate response with lower match_index.
        raft.handle_append_entries_response(create_append_entries_response(0, 2, true, 3));

        let ls = raft.leader_state.as_ref().unwrap();
        assert_eq!(ls.match_index[0], 5, "match_index must not go backwards");
        assert_eq!(ls.next_index[0], 6, "next_index must not go backwards");
    }

    // ── commit index and apply ────────────────────────────────────────────────

    #[test]
    fn commit_index_advances_when_quorum_reached() {
        // 3-node cluster (leader + 2 peers). Quorum = 2.
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 1;
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);

        // Peer 2 confirms up to index 2.
        raft.handle_append_entries_response(create_append_entries_response(1, 2, true, 2));

        // Leader (self) has index 2, peer 2 has index 2 → quorum of 2 reached.
        assert_eq!(raft.volatile.commit_index, 2);
    }

    #[test]
    fn commit_index_does_not_advance_without_quorum() {
        // 5-node cluster (leader + 4 peers). Quorum = 3.
        let mut raft = make_leader(1, &[2, 3, 4, 5]);
        raft.persistent.current_term = 1;
        push_entry(&mut raft, 1, 1);

        // Only peer 2 has confirmed.
        raft.handle_append_entries_response(create_append_entries_response(1, 2, true, 1));

        // leader + peer2 = 2, quorum = 3 → not yet.
        assert_eq!(raft.volatile.commit_index, 0);
    }

    #[test]
    fn commit_index_advances_only_for_current_term_entries() {
        // §5.4: a leader must not commit entries from previous terms by
        // counting replicas — only entries from the *current* term.
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 3;
        push_entry(&mut raft, 1, 1); // old term
        push_entry(&mut raft, 2, 1); // old term

        raft.handle_append_entries_response(create_append_entries_response(3, 2, true, 2));

        assert_eq!(
            raft.volatile.commit_index, 0,
            "must not commit entries from previous terms"
        );
    }

    #[test]
    fn old_term_entries_committed_indirectly_via_current_term_entry() {
        // Classic §5.4 scenario: entries from old terms get committed when a
        // current-term entry reaches quorum and the commit index passes them.
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 3;
        push_entry(&mut raft, 1, 1); // old-term entry
        push_entry(&mut raft, 2, 3); // current-term entry

        // Peer 2 has both.
        raft.handle_append_entries_response(create_append_entries_response(3, 2, true, 2));

        // Index 2 (current term) reaches quorum → commit index = 2.
        // Index 1 (old term) is implicitly committed because 1 <= 2.
        assert_eq!(raft.volatile.commit_index, 2);
    }

    #[test]
    fn apply_actions_emitted_after_commit_index_advances() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 1;
        push_entry(&mut raft, 1, 1);
        push_entry(&mut raft, 2, 1);

        let actions =
            raft.handle_append_entries_response(create_append_entries_response(1, 2, true, 2));

        let apply_count = actions
            .iter()
            .filter(|a| matches!(a, Action::ApplyCommand(_)))
            .count();
        assert_eq!(apply_count, 2);
        assert_eq!(raft.volatile.last_applied, 2);
    }

    #[test]
    fn no_apply_actions_when_commit_index_unchanged() {
        let mut raft = make_leader(1, &[2, 3, 4, 5]);
        raft.persistent.current_term = 1;
        push_entry(&mut raft, 1, 1);

        // Only 1 peer out of 4 → no quorum (need 3).
        let actions =
            raft.handle_append_entries_response(create_append_entries_response(1, 2, true, 1));

        assert!(!actions.iter().any(|a| matches!(a, Action::ApplyCommand(_))));
    }

    #[test]
    fn multiple_peers_confirming_same_index_does_not_double_apply() {
        let mut raft = make_leader(1, &[2, 3]);
        raft.persistent.current_term = 1;
        push_entry(&mut raft, 1, 1);

        raft.handle_append_entries_response(create_append_entries_response(1, 2, true, 1)); // commits
        let actions =
            raft.handle_append_entries_response(create_append_entries_response(1, 3, true, 1)); // already committed

        let apply_count = actions
            .iter()
            .filter(|a| matches!(a, Action::ApplyCommand(_)))
            .count();
        assert_eq!(apply_count, 0, "must not apply the same entry twice");
    }

    // ════════════════════════════════════════════════════════════════════════
    // helpers
    // ════════════════════════════════════════════════════════════════════════

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

    /// Build an AppendEntries message.
    fn create_append_entries(
        term: u64,
        leader_id: NodeId,
        prev_index: u64,
        prev_term: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
    ) -> AppendEntries {
        AppendEntries {
            term,
            leader_id,
            prev_log_index: prev_index,
            prev_log_term: prev_term,
            entries,
            leader_commit,
        }
    }

    /// Build an AppendEntriesResponse.
    fn create_append_entries_response(
        term: u64,
        from: NodeId,
        success: bool,
        match_index: u64,
    ) -> AppendEntriesResponse {
        AppendEntriesResponse {
            term,
            from,
            success,
            match_index,
        }
    }

    fn create_log_entry(index: u64, term: u64) -> LogEntry {
        LogEntry {
            index,
            term,
            command: Command::default(),
        }
    }

    fn response_success(actions: &[Action]) -> bool {
        actions
            .iter()
            .any(|a| matches!(a, Action::Send(_, Message::AppendEntriesResponse(r)) if r.success))
    }

    fn response_term(actions: &[Action]) -> u64 {
        actions
            .iter()
            .find_map(|a| match a {
                Action::Send(_, Message::AppendEntriesResponse(r)) => Some(r.term),
                _ => None,
            })
            .expect("no AppendEntriesResponse found")
    }

    fn has_election_timer_reset(actions: &[Action]) -> bool {
        actions
            .iter()
            .any(|a| matches!(a, Action::ResetElectionTimer))
    }
}
