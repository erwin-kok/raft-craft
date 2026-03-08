use crate::{
    Raft,
    protocol::{
        action::Action,
        message::{Message, RequestVote, RequestVoteResponse},
        types::NodeId,
    },
};

impl Raft {
    pub(crate) fn handle_request_vote(&mut self, msg: RequestVote) -> Vec<Action> {
        // wrong term --> not granted (candidate is stale)
        if msg.term < self.persistent.current_term {
            return vec![self.create_vote_response_action(msg.candidate_id, false)];
        }

        // check whether candidate has higher term
        // if so: update term, convert to follower, reset voted_for
        // and continue vote processing.
        if msg.term > self.persistent.current_term {
            self.persistent.current_term = msg.term;
            self.role = crate::Role::Follower;
            self.persistent.voted_for = None;
        }

        // check already voted
        if let Some(voted_for) = self.persistent.voted_for
            && voted_for != msg.candidate_id
        {
            return vec![self.create_vote_response_action(msg.candidate_id, false)];
        }

        // check log up to date
        if !self.candidate_log_up_to_date(&msg) {
            return vec![self.create_vote_response_action(msg.candidate_id, false)];
        }

        // vote granted
        self.persistent.voted_for = Some(msg.candidate_id);
        vec![
            Action::ResetElectionTimer,
            self.create_vote_response_action(msg.candidate_id, true),
        ]
    }

    pub(crate) fn handle_vote_response(&mut self, msg: RequestVoteResponse) -> Vec<Action> {
        // wrong term --> ignore
        if msg.term < self.persistent.current_term {
            return vec![];
        }

        // do nothing when not candidate
        if self.role != crate::Role::Candidate {
            return vec![];
        }

        // reject vote from unknown peer
        if !self.peers.contains(&msg.vote_from) {
            return vec![];
        }

        // discovered higher term -> step down
        if msg.term > self.persistent.current_term {
            self.persistent.current_term = msg.term;
            self.role = crate::Role::Follower;
            self.persistent.voted_for = None;
            self.persistent.votes_from.clear();
            self.persistent.votes_granted = 0;
            return vec![];
        }

        // increase number of votes received, and, if granted, also number of votes granted
        if !self.persistent.votes_from.insert(msg.vote_from) {
            return vec![]; // duplicate vote response
        }
        if msg.vote_granted {
            self.persistent.votes_granted += 1;
        }

        // calculate majority
        let majority = ((self.peers.len() / 2) + 1) as u64;

        // if not yet won, continue waiting.
        if self.persistent.votes_granted < majority {
            return vec![];
        }

        // we won: become leader and initialize statistics.
        self.become_leader();

        // Note: do not reset election timer.
        vec![Action::ResetHeartbeatTimer]
    }

    fn candidate_log_up_to_date(&self, msg: &RequestVote) -> bool {
        let (last_log_index, last_log_term) = self
            .persistent
            .log
            .last()
            .map(|e| (e.index, e.term))
            .unwrap_or((0, 0)); // empty log defaults

        if msg.last_log_term > last_log_term {
            return true;
        }

        if msg.last_log_term == last_log_term && msg.last_log_index >= last_log_index {
            return true;
        }

        false
    }

    fn create_vote_response_action(&self, candidate_id: NodeId, granted: bool) -> Action {
        let vote = RequestVoteResponse {
            term: self.persistent.current_term,
            vote_granted: granted,
            vote_from: self.id,
        };
        Action::Send(candidate_id, Message::RequestVoteResponse(vote))
    }

    pub(crate) fn become_leader(&mut self) {
        let last_log_index = self.persistent.log.last().map(|e| e.index).unwrap_or(0);

        self.role = crate::Role::Leader;
        self.leader_state = Some(crate::LeaderState {
            next_index: vec![last_log_index + 1; self.peers.len()],
            match_index: vec![0; self.peers.len()],
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Raft,
        protocol::{
            action::Action,
            command::Command,
            log::LogEntry,
            message::{Message, RequestVote, RequestVoteResponse},
            types::NodeId,
        },
    };

    #[test]
    fn reject_vote_if_candidate_term_is_stale() {
        let mut raft = new_node();
        raft.persistent.current_term = 5;

        let msg = RequestVote {
            term: 4,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(!resp.vote_granted);
        assert_eq!(resp.term, 5);
        assert!(!has_reset_timer(&actions));
    }

    #[test]
    fn higher_term_updates_current_term_and_steps_down() {
        let mut raft = new_node();

        raft.persistent.current_term = 2;
        raft.role = crate::Role::Leader;
        raft.persistent.voted_for = Some(3);

        let msg = RequestVote {
            term: 5,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };

        let _ = raft.handle_request_vote(msg);

        assert_eq!(raft.persistent.current_term, 5);
        assert_eq!(raft.role, crate::Role::Follower);
        assert_eq!(raft.persistent.voted_for, Some(2));
    }

    #[test]
    fn reject_if_already_voted_for_other_candidate() {
        let mut raft = new_node();

        raft.persistent.current_term = 3;
        raft.persistent.voted_for = Some(10);

        let msg = RequestVote {
            term: 3,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(!resp.vote_granted);
        assert!(!has_reset_timer(&actions));
    }

    #[test]
    fn allow_same_candidate_retry() {
        let mut raft = new_node();

        raft.persistent.current_term = 3;
        raft.persistent.voted_for = Some(2);

        let msg = RequestVote {
            term: 3,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(resp.vote_granted);
        assert!(has_reset_timer(&actions));
    }

    #[test]
    fn reject_if_candidate_log_term_is_older() {
        let mut raft = new_node();

        raft.persistent.log.push(LogEntry {
            index: 5,
            term: 3,
            command: Command::default(),
        });

        let msg = RequestVote {
            term: 4,
            candidate_id: 2,
            last_log_index: 10,
            last_log_term: 2,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(!resp.vote_granted);
    }

    #[test]
    fn reject_if_same_term_but_shorter_log() {
        let mut raft = new_node();

        raft.persistent.log.push(LogEntry {
            index: 10,
            term: 3,
            command: Command::default(),
        });

        let msg = RequestVote {
            term: 4,
            candidate_id: 2,
            last_log_index: 5,
            last_log_term: 3,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(!resp.vote_granted);
    }

    #[test]
    fn accept_if_candidate_log_term_newer() {
        let mut raft = new_node();

        raft.persistent.log.push(LogEntry {
            index: 10,
            term: 2,
            command: Command::default(),
        });

        let msg = RequestVote {
            term: 3,
            candidate_id: 2,
            last_log_index: 1,
            last_log_term: 5,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(resp.vote_granted);
    }

    #[test]
    fn accept_if_same_term_but_longer_log() {
        let mut raft = new_node();

        raft.persistent.log.push(LogEntry {
            index: 5,
            term: 3,
            command: Command::default(),
        });

        let msg = RequestVote {
            term: 4,
            candidate_id: 2,
            last_log_index: 10,
            last_log_term: 3,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(resp.vote_granted);
    }

    #[test]
    fn accept_if_logs_identical() {
        let mut raft = new_node();

        raft.persistent.log.push(LogEntry {
            index: 5,
            term: 3,
            command: Command::default(),
        });

        let msg = RequestVote {
            term: 4,
            candidate_id: 2,
            last_log_index: 5,
            last_log_term: 3,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(resp.vote_granted);
    }

    #[test]
    fn accept_if_follower_log_empty() {
        let mut raft = new_node();

        let msg = RequestVote {
            term: 1,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(resp.vote_granted);
    }

    #[test]
    fn timer_reset_only_on_vote_granted() {
        let mut raft = new_node();

        raft.persistent.voted_for = Some(9);

        let msg = RequestVote {
            term: raft.persistent.current_term,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };

        let actions = raft.handle_request_vote(msg);

        assert!(!has_reset_timer(&actions));
    }

    #[test]
    fn never_vote_twice_in_same_term() {
        let mut raft = new_node();

        raft.persistent.current_term = 4;

        let first = RequestVote {
            term: 4,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };

        let second = RequestVote {
            term: 4,
            candidate_id: 3,
            last_log_index: 0,
            last_log_term: 0,
        };

        let a1 = raft.handle_request_vote(first);
        let r1 = extract_vote_response(&a1);

        let a2 = raft.handle_request_vote(second);
        let r2 = extract_vote_response(&a2);

        assert!(r1.vote_granted);
        assert!(!r2.vote_granted);
    }

    #[test]
    fn higher_term_clears_previous_vote() {
        let mut raft = new_node();

        raft.persistent.current_term = 3;
        raft.persistent.voted_for = Some(10);

        let msg = RequestVote {
            term: 4,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(resp.vote_granted);
        assert_eq!(raft.persistent.current_term, 4);
        assert_eq!(raft.persistent.voted_for, Some(2));
    }

    #[test]
    fn leader_steps_down_on_higher_term_vote_request() {
        let mut raft = new_node();

        raft.role = crate::Role::Leader;
        raft.persistent.current_term = 5;

        let msg = RequestVote {
            term: 6,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        };

        raft.handle_request_vote(msg);

        assert_eq!(raft.role, crate::Role::Follower);
        assert_eq!(raft.persistent.current_term, 6);
    }

    #[test]
    fn log_term_takes_priority_over_log_length() {
        let mut raft = new_node();

        raft.persistent.log.push(LogEntry {
            index: 100,
            term: 9,
            command: Command::default(),
        });

        let msg = RequestVote {
            term: 11,
            candidate_id: 2,
            last_log_index: 5,
            last_log_term: 10,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(resp.vote_granted);
    }

    #[test]
    fn reject_if_same_index_but_older_term() {
        let mut raft = new_node();

        raft.persistent.log.push(LogEntry {
            index: 10,
            term: 5,
            command: Command::default(),
        });

        let msg = RequestVote {
            term: 6,
            candidate_id: 2,
            last_log_index: 10,
            last_log_term: 4,
        };

        let actions = raft.handle_request_vote(msg);

        let resp = extract_vote_response(&actions);

        assert!(!resp.vote_granted);
    }

    #[test]
    fn ignore_stale_vote_response() {
        let mut raft = new_node();

        raft.persistent.current_term = 5;

        let msg = RequestVoteResponse {
            term: 4,
            vote_granted: true,
            vote_from: 1,
        };

        let actions = raft.handle_vote_response(msg);

        assert!(actions.is_empty());
    }

    #[test]
    fn become_leader_on_majority_votes() {
        let mut raft = new_node_with_peers(1, vec![1, 2, 3]);

        raft.role = crate::Role::Candidate;
        raft.persistent.votes_granted = 2;
        raft.persistent.votes_from.extend([1, 2]);

        let msg = RequestVoteResponse {
            term: raft.persistent.current_term,
            vote_granted: true,
            vote_from: 3,
        };

        let actions = raft.handle_vote_response(msg);

        assert_eq!(raft.role, crate::Role::Leader);
        assert!(matches!(actions[0], Action::ResetHeartbeatTimer));
    }

    #[test]
    fn ignore_vote_response_when_not_candidate() {
        let mut raft = new_node();
        raft.role = crate::Role::Follower;

        let msg = RequestVoteResponse {
            term: raft.persistent.current_term,
            vote_granted: true,
            vote_from: 1,
        };

        let actions = raft.handle_vote_response(msg);
        assert!(actions.is_empty());
    }

    #[test]
    fn step_down_on_higher_term_vote_response() {
        let mut raft = new_node();
        raft.role = crate::Role::Candidate;
        raft.persistent.current_term = 5;
        raft.persistent.votes_from.extend([1, 2]);
        raft.persistent.votes_granted = 2;

        let msg = RequestVoteResponse {
            term: 6, // higher than current
            vote_granted: true,
            vote_from: 1,
        };

        let actions = raft.handle_vote_response(msg);

        assert_eq!(raft.role, crate::Role::Follower);
        assert_eq!(raft.persistent.current_term, 6);
        assert!(raft.persistent.voted_for.is_none());
        assert_eq!(raft.persistent.votes_from.len(), 0);
        assert_eq!(raft.persistent.votes_granted, 0);
        assert!(actions.is_empty());
    }

    #[test]
    fn continue_waiting_if_majority_not_reached() {
        let mut raft = new_node_with_peers(1, vec![1, 2, 3, 4, 5]);
        raft.role = crate::Role::Candidate;
        raft.persistent.current_term = 3;
        raft.persistent.votes_from.extend([1, 2]);
        raft.persistent.votes_granted = 1; // only 1 granted, majority=3

        let msg = RequestVoteResponse {
            term: 3,
            vote_granted: true,
            vote_from: 3,
        };

        let actions = raft.handle_vote_response(msg);

        // now votes_received = 3, votes_granted = 2, majority=3 -> still waiting
        assert_eq!(raft.persistent.votes_from.len(), 3);
        assert_eq!(raft.persistent.votes_granted, 2);
        assert_eq!(raft.role, crate::Role::Candidate);
        assert!(actions.is_empty());
    }

    #[test]
    fn delayed_vote_response_after_leader_is_ignored() {
        let mut raft = new_node();
        raft.role = crate::Role::Leader;

        let msg = RequestVoteResponse {
            term: raft.persistent.current_term,
            vote_granted: true,
            vote_from: 1,
        };

        let actions = raft.handle_vote_response(msg);
        assert!(actions.is_empty());
    }

    #[test]
    fn leader_state_initialization_on_becoming_leader() {
        let mut raft = new_node_with_peers(1, vec![1, 2, 3]);
        raft.role = crate::Role::Candidate;
        raft.persistent.current_term = 1;
        raft.persistent.votes_from.extend([1, 2]);
        raft.persistent.votes_granted = 2;

        let msg = RequestVoteResponse {
            term: 1,
            vote_granted: true,
            vote_from: 3,
        };

        let actions = raft.handle_vote_response(msg);

        assert_eq!(raft.role, crate::Role::Leader);
        assert!(matches!(actions[0], Action::ResetHeartbeatTimer));

        let leader_state = raft.leader_state.as_ref().unwrap();
        let last_log_index = raft.persistent.log.last().map(|e| e.index).unwrap_or(0);
        assert_eq!(
            leader_state.next_index,
            vec![last_log_index + 1; raft.peers.len()]
        );
        assert_eq!(leader_state.match_index, vec![0; raft.peers.len()]);
    }

    #[test]
    fn majority_calculation_odd_and_even_cluster() {
        // odd cluster
        let raft = new_node_with_peers(1, vec![1, 2, 3, 4, 5]);
        let majority = raft.peers.len() / 2 + 1;
        assert_eq!(majority, 3);

        // even cluster
        let raft2 = new_node_with_peers(1, vec![1, 2, 3, 4]);
        let majority2 = raft2.peers.len() / 2 + 1;
        assert_eq!(majority2, 3);
    }

    #[test]
    fn candidate_wins_even_with_some_rejections() {
        let mut raft = new_node_with_peers(1, vec![1, 2, 3, 4, 5]);
        raft.role = crate::Role::Candidate;
        raft.persistent.votes_from.extend([1, 2]);
        raft.persistent.votes_granted = 2;

        // rejection
        raft.handle_vote_response(RequestVoteResponse {
            term: raft.persistent.current_term,
            vote_granted: false,
            vote_from: 3,
        });

        // still waiting
        assert_eq!(raft.role, crate::Role::Candidate);

        // grant
        let actions = raft.handle_vote_response(RequestVoteResponse {
            term: raft.persistent.current_term,
            vote_granted: true,
            vote_from: 4,
        });

        assert_eq!(raft.role, crate::Role::Leader);
        assert!(matches!(actions[0], Action::ResetHeartbeatTimer));
    }

    #[test]
    fn candidate_wins_on_exact_majority() {
        let mut raft = new_node_with_peers(1, vec![1, 2, 3]);

        raft.role = crate::Role::Candidate;
        raft.persistent.votes_from.insert(1);
        raft.persistent.votes_granted = 1;

        let actions = raft.handle_vote_response(RequestVoteResponse {
            term: raft.persistent.current_term,
            vote_granted: true,
            vote_from: 2,
        });

        assert_eq!(raft.role, crate::Role::Leader);
        assert!(matches!(actions[0], Action::ResetHeartbeatTimer));
    }

    #[test]
    fn rejected_vote_does_not_increase_granted() {
        let mut raft = new_node();
        raft.role = crate::Role::Candidate;

        raft.persistent.votes_from.insert(1);
        raft.persistent.votes_granted = 1;

        raft.handle_vote_response(RequestVoteResponse {
            term: raft.persistent.current_term,
            vote_granted: false,
            vote_from: 1,
        });

        assert!(raft.persistent.votes_from.contains(&1));
        assert_eq!(raft.persistent.votes_granted, 1);
    }

    #[test]
    fn leader_transition_happens_only_once() {
        let mut raft = new_node_with_peers(1, vec![1, 2, 3, 4]);

        raft.role = crate::Role::Candidate;
        raft.persistent.votes_from.insert(2);
        raft.persistent.votes_granted = 2;

        raft.handle_vote_response(RequestVoteResponse {
            term: raft.persistent.current_term,
            vote_granted: true,
            vote_from: 3,
        });

        assert_eq!(raft.role, crate::Role::Leader);

        let actions = raft.handle_vote_response(RequestVoteResponse {
            term: raft.persistent.current_term,
            vote_granted: true,
            vote_from: 4,
        });

        assert!(actions.is_empty());
    }

    fn extract_vote_response(actions: &[Action]) -> &RequestVoteResponse {
        actions
            .iter()
            .find_map(|a| match a {
                Action::Send(_, Message::RequestVoteResponse(v)) => Some(v),
                _ => None,
            })
            .expect("vote response missing")
    }

    fn has_reset_timer(actions: &[Action]) -> bool {
        actions
            .iter()
            .any(|a| matches!(a, Action::ResetElectionTimer))
    }

    /// Helper to create a Raft node with peers
    fn new_node() -> Raft {
        Raft::new(1, vec![1, 2, 3])
    }

    fn new_node_with_peers(id: NodeId, peers: Vec<NodeId>) -> Raft {
        Raft::new(id, peers)
    }
}
