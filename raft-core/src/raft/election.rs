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
        protocol::{action::Action, message::Message},
    };

    #[test]
    fn election_timeout_promotes_follower_to_candidate_and_sends_votes() {
        let mut raft = new_node();

        // ensure starting as follower
        assert_eq!(raft.role, crate::Role::Follower);
        assert_eq!(raft.persistent.current_term, 0);
        assert!(raft.persistent.voted_for.is_none());

        // call election timeout
        let actions = raft.handle_election_timeout();

        // ---------------------------
        // Check Raft state updates
        // ---------------------------
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

        // ---------------------------
        // Check actions returned
        // ---------------------------
        // Should include one ResetElectionTimer action
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::ResetElectionTimer))
        );

        // Should include vote messages to all other peers
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
    fn election_timeout_does_nothing_if_leader() {
        let mut raft = new_node();
        raft.role = crate::Role::Leader;

        let actions = raft.handle_election_timeout();

        // Leader should not change term or role
        assert_eq!(raft.role, crate::Role::Leader);
        assert_eq!(raft.persistent.current_term, 0);
        assert!(raft.persistent.voted_for.is_none());

        // No actions
        assert!(actions.is_empty());
    }

    fn new_node() -> Raft {
        Raft::new(1, vec![1, 2, 3])
    }
}
