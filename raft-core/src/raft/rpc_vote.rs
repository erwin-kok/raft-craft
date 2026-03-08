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

    pub(crate) fn handle_vote_response(&mut self, _msg: RequestVoteResponse) -> Vec<Action> {
        vec![]
    }

    fn candidate_log_up_to_date(&self, msg: &RequestVote) -> bool {
        let last_log_entry = self.persistent.log.last();
        let (last_log_index, last_log_term) = match last_log_entry {
            Some(entry) => (entry.index, entry.term),
            None => (0, 0), // empty log defaults
        };

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
        };
        Action::Send(candidate_id, Message::RequestVoteResponse(vote))
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
        Raft::new(1, vec![2, 3])
    }
}
