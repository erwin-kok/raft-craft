use crate::{
    Raft,
    protocol::{action::Action, message::Message, message::RequestVote, types::NodeId},
};

impl Raft {
    pub(crate) fn handle_election_timeout(&mut self) -> Vec<Action> {
        // do nothing when already leader
        if self.role == crate::Role::Leader {
            return vec![];
        }

        // become candidate
        self.role = crate::Role::Candidate;

        // increment current term
        self.persistent.current_term += 1;

        // vote for self
        self.persistent.voted_for = Some(self.id);
        self.persistent.votes_from.clear();
        self.persistent.votes_from.insert(self.id);
        self.persistent.votes_granted = 1;

        // If there is only one peer (ourself), immediately become leader.
        if self.peers.len() == 1 {
            self.become_leader();
            return vec![Action::ResetHeartbeatTimer];
        }

        // restart election timer
        let mut actions = vec![Action::ResetElectionTimer];

        // send vote requests to all other peers
        actions.extend(
            self.peers
                .iter()
                .filter(|&&peer_id| peer_id != self.id)
                .map(|peer_id| self.create_vote_action(*peer_id)),
        );

        actions
    }

    fn create_vote_action(&self, peer_id: NodeId) -> Action {
        let last_log_entry = self.persistent.log.last();
        let (last_log_index, last_log_term) = match last_log_entry {
            Some(entry) => (entry.index, entry.term),
            None => (0, 0), // empty log defaults
        };
        let vote = RequestVote {
            term: self.persistent.current_term,
            candidate_id: self.id,
            last_log_index,
            last_log_term,
        };
        Action::Send(peer_id, Message::RequestVote(vote))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Raft,
        protocol::{action::Action, message::Message, types::NodeId},
    };

    #[test]
    fn election_timeout_promotes_follower_to_candidate_and_sends_votes() {
        let mut raft = new_node_with_peers(1, vec![1, 2, 3]);

        // ensure starting as follower
        assert_eq!(raft.role, crate::Role::Follower);
        assert_eq!(raft.persistent.current_term, 0);
        assert!(raft.persistent.voted_for.is_none());

        // call election timeout
        let actions = raft.handle_election_timeout();

        // Check state updates
        assert_eq!(
            raft.role,
            crate::Role::Candidate,
            "Follower should become Candidate"
        );
        assert_eq!(raft.persistent.current_term, 1, "Term should increment");
        assert_eq!(
            raft.persistent.voted_for,
            Some(raft.id),
            "Candidate votes for self"
        );

        // Check actions
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::ResetElectionTimer))
        );
        for peer_id in raft.peers.iter().filter(|&&id| id != raft.id) {
            assert!(actions.iter().any(|a| {
                matches!(a, Action::Send(id, Message::RequestVote(vote)) if vote.candidate_id == raft.id && vote.term == raft.persistent.current_term)
            }), "Missing vote message for peer {}", peer_id);
        }

        // No extra vote messages
        let vote_count = actions
            .iter()
            .filter(|a| matches!(a, Action::Send(_, Message::RequestVote(_))))
            .count();
        assert_eq!(vote_count, raft.peers.len() - 1);
    }

    #[test]
    fn election_timeout_with_empty_log_defaults_to_zero() {
        let mut raft = new_node_with_peers(1, vec![2, 3]);
        raft.persistent.log.clear(); // ensure empty

        let actions = raft.handle_election_timeout();

        for a in actions.iter() {
            if let Action::Send(_, Message::RequestVote(vote)) = a {
                assert_eq!(vote.last_log_index, 0);
                assert_eq!(vote.last_log_term, 0);
            }
        }
    }

    #[test]
    fn election_timeout_for_candidate_increments_term() {
        let mut raft = new_node_with_peers(1, vec![2, 3]);
        raft.role = crate::Role::Candidate;
        raft.persistent.current_term = 5;

        let actions = raft.handle_election_timeout();

        assert_eq!(raft.role, crate::Role::Candidate);
        assert_eq!(raft.persistent.current_term, 6);
        assert_eq!(raft.persistent.voted_for, Some(raft.id));

        let vote_count = actions
            .iter()
            .filter(|a| matches!(a, Action::Send(_, Message::RequestVote(_))))
            .count();
        assert_eq!(vote_count, 2);
    }

    #[test]
    fn election_timeout_does_nothing_if_leader() {
        let mut raft = new_node_with_peers(1, vec![2, 3]);
        raft.role = crate::Role::Leader;
        raft.persistent.current_term = 3;

        let actions = raft.handle_election_timeout();

        // Leader should not change term or role
        assert_eq!(raft.role, crate::Role::Leader);
        assert_eq!(raft.persistent.current_term, 3);
        assert!(raft.persistent.voted_for.is_none());

        // No actions
        assert!(actions.is_empty());
    }

    #[test]
    fn election_timeout_sends_request_vote_to_all_other_peers() {
        let mut raft = new_node_with_peers(1, vec![1, 2, 3, 4, 5]);

        let actions = raft.handle_election_timeout();

        let vote_peers: Vec<NodeId> = actions
            .iter()
            .filter_map(|a| {
                if let Action::Send(peer_id, Message::RequestVote(_)) = a {
                    Some(*peer_id)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(vote_peers.len(), 4); // all peers except self
        assert!(!vote_peers.contains(&raft.id));
        assert!(vote_peers.contains(&2));
        assert!(vote_peers.contains(&3));
        assert!(vote_peers.contains(&4));
        assert!(vote_peers.contains(&5));
    }

    #[test]
    fn election_timeout_returns_one_reset_timer() {
        let mut raft = new_node_with_peers(1, vec![2, 3, 4]);
        let actions = raft.handle_election_timeout();

        let reset_count = actions
            .iter()
            .filter(|a| matches!(a, Action::ResetElectionTimer))
            .count();
        assert_eq!(reset_count, 1);
    }

    #[test]
    fn single_node_cluster_becomes_leader_immediately() {
        let mut raft = new_node_with_peers(1, vec![1]); // no other peers
        let actions = raft.handle_election_timeout();

        assert_eq!(actions.len(), 1);

        assert_eq!(raft.role, crate::Role::Leader);
        assert!(matches!(actions[0], Action::ResetHeartbeatTimer));
    }

    #[test]
    fn candidate_restart_election_resets_votes() {
        let mut raft = new_node_with_peers(1, vec![2, 3]);
        raft.role = crate::Role::Candidate;
        raft.persistent.current_term = 3;
        raft.persistent.voted_for = Some(99);
        raft.persistent.votes_granted = 99;
        raft.persistent.votes_from.extend([1, 2, 3, 4, 5]);

        let actions = raft.handle_election_timeout();

        assert_eq!(raft.role, crate::Role::Candidate);
        assert_eq!(raft.persistent.current_term, 4);
        assert_eq!(raft.persistent.voted_for, Some(raft.id));
        assert!(raft.persistent.votes_from.contains(&1));
        assert_eq!(raft.persistent.votes_granted, 1);

        let vote_count = actions
            .iter()
            .filter(|a| matches!(a, Action::Send(_, Message::RequestVote(_))))
            .count();
        assert_eq!(vote_count, 2);
    }

    #[test]
    fn multiple_election_timeouts_only_one_reset_per_call() {
        let mut raft = new_node_with_peers(1, vec![2, 3]);
        let actions1 = raft.handle_election_timeout();
        let actions2 = raft.handle_election_timeout();

        let reset1 = actions1
            .iter()
            .filter(|a| matches!(a, Action::ResetElectionTimer))
            .count();
        let reset2 = actions2
            .iter()
            .filter(|a| matches!(a, Action::ResetElectionTimer))
            .count();

        assert_eq!(reset1, 1);
        assert_eq!(reset2, 1);
    }

    /// Helper to create a Raft node with peers
    fn new_node_with_peers(id: NodeId, peers: Vec<NodeId>) -> Raft {
        Raft::new(id, peers)
    }
}
