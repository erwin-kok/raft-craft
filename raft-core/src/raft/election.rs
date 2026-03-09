use crate::{
    CandidateState, Raft, Role,
    protocol::{action::Action, message::Message, message::RequestVote, types::NodeId},
};

impl Raft {
    pub(crate) fn handle_election_timeout(&mut self) -> Vec<Action> {
        // do nothing when already leader
        if self.role == Role::Leader {
            return vec![];
        }

        // become candidate
        self.role = Role::Candidate;

        // increment current term
        self.persistent.current_term += 1;

        // vote for self
        self.persistent.voted_for = Some(self.id);

        // ensure leader state is none.
        self.leader_state = None;

        // Start fresh election state; self-vote is pre-counted.
        let mut state = CandidateState::default();
        state.votes_received.insert(self.id);
        state.votes_granted.insert(self.id);
        self.candidate_state = Some(state);

        // Single-node cluster: win immediately.
        if self.peers.is_empty() {
            self.become_leader();
            return vec![Action::ResetHeartbeatTimer];
        }

        // restart election timer
        let mut actions = vec![Action::ResetElectionTimer];

        // send vote requests to all other peers
        actions.extend(
            self.peers
                .iter()
                .map(|&peer_id| self.create_vote_action(peer_id)),
        );
        actions
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn create_vote_action(&self, peer_id: NodeId) -> Action {
        let vote = RequestVote {
            term: self.persistent.current_term,
            candidate_id: self.id,
            last_log_index: self.last_log_index(),
            last_log_term: self.last_log_term(),
        };
        Action::Send(peer_id, Message::RequestVote(vote))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        CandidateState, Raft, Role,
        protocol::{action::Action, message::Message, types::NodeId},
    };

    #[test]
    fn election_timeout_promotes_follower_to_candidate_and_sends_votes() {
        let mut raft = new_raft(1, &[2, 3]);

        // ensure starting as follower
        assert_eq!(raft.role, Role::Follower);
        assert_eq!(raft.persistent.current_term, 0);
        assert!(raft.persistent.voted_for.is_none());

        // call election timeout
        let actions = raft.handle_election_timeout();

        // Check state updates
        assert_eq!(raft.role, Role::Candidate);
        assert_eq!(raft.persistent.current_term, 1);
        assert_eq!(raft.persistent.voted_for, Some(raft.id));

        // Check actions
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::ResetElectionTimer))
        );

        for &peer_id in &raft.peers {
            assert!(
                actions.iter().any(|a| matches!(
                    a,
                    Action::Send(id, Message::RequestVote(v))
                    if *id == peer_id && v.candidate_id == raft.id && v.term == raft.persistent.current_term
                )),
                "missing RequestVote for peer {peer_id}"
            );
        }

        let vote_count = actions
            .iter()
            .filter(|a| matches!(a, Action::Send(_, Message::RequestVote(_))))
            .count();
        assert_eq!(vote_count, raft.peers.len());
    }

    #[test]
    fn election_timeout_with_empty_log_sends_zero_indices() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.log.clear(); // ensure empty

        let actions = raft.handle_election_timeout();

        for a in &actions {
            if let Action::Send(_, Message::RequestVote(v)) = a {
                assert_eq!(v.last_log_index, 0);
                assert_eq!(v.last_log_term, 0);
            }
        }
    }

    #[test]
    fn election_timeout_for_candidate_increments_term() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Candidate;
        raft.persistent.current_term = 5;

        let actions = raft.handle_election_timeout();

        assert_eq!(raft.role, Role::Candidate);
        assert_eq!(raft.persistent.current_term, 6);
        assert_eq!(raft.persistent.voted_for, Some(raft.id));
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, Action::Send(_, Message::RequestVote(_))))
                .count(),
            2
        );
    }

    #[test]
    fn election_timeout_does_nothing_if_leader() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Leader;
        raft.persistent.current_term = 3;

        let actions = raft.handle_election_timeout();

        // Leader should not change term or role
        assert_eq!(raft.role, Role::Leader);
        assert_eq!(raft.persistent.current_term, 3);
        assert!(raft.persistent.voted_for.is_none());

        // No actions
        assert!(actions.is_empty());
    }

    #[test]
    fn election_timeout_sends_vote_to_all_peers() {
        let mut raft = new_raft(1, &[2, 3, 4, 5]);

        let actions = raft.handle_election_timeout();

        let voted_peers: Vec<NodeId> = actions
            .iter()
            .filter_map(|a| {
                if let Action::Send(peer_id, Message::RequestVote(_)) = a {
                    Some(*peer_id)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(voted_peers.len(), 4);
        assert!(!voted_peers.contains(&1)); // self not in peers, never messaged
        for &p in &[2u64, 3, 4, 5] {
            assert!(voted_peers.contains(&p));
        }
    }

    #[test]
    fn election_timeout_returns_one_reset_timer() {
        let mut raft = new_raft(1, &[2, 3, 4]);
        let actions = raft.handle_election_timeout();

        let count = actions
            .iter()
            .filter(|a| matches!(a, Action::ResetElectionTimer))
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn single_node_cluster_becomes_leader_immediately() {
        let mut raft = new_raft(1, &[]);

        let actions = raft.handle_election_timeout();

        assert_eq!(raft.role, Role::Leader);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::ResetHeartbeatTimer));
    }

    #[test]
    fn candidate_restart_election_resets_votes() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Candidate;
        raft.persistent.current_term = 3;
        raft.persistent.voted_for = Some(99);

        // Inject stale candidate state.
        let mut stale = CandidateState::default();
        stale.votes_received.extend([1, 2, 3, 4, 5]);
        stale.votes_granted.extend([1, 2, 3, 4, 5]);
        raft.candidate_state = Some(stale);

        raft.handle_election_timeout();

        assert_eq!(raft.persistent.current_term, 4);
        assert_eq!(raft.persistent.voted_for, Some(raft.id));

        let cs = raft.candidate_state.as_ref().unwrap();
        assert_eq!(cs.votes_granted.len(), 1, "only self-vote should remain");
        assert!(cs.votes_granted.contains(&1));
    }

    #[test]
    fn multiple_calls_produce_one_reset_timer_each() {
        let mut raft = new_raft(1, &[2, 3]);

        for _ in 0..3 {
            let actions = raft.handle_election_timeout();
            let count = actions
                .iter()
                .filter(|a| matches!(a, Action::ResetElectionTimer))
                .count();
            assert_eq!(count, 1);
        }
    }

    fn new_raft(id: NodeId, peers: &[NodeId]) -> Raft {
        Raft::new(id, peers.to_vec())
    }
}
