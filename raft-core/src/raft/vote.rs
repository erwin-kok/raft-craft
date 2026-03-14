use crate::{
    LeaderState, Raft, Role,
    protocol::{
        action::Action,
        message::{Message, RequestVote, RequestVoteResponse},
        types::{NodeId, Term},
    },
};

impl Raft {
    pub(crate) fn handle_request_vote(&mut self, msg: RequestVote) -> Vec<Action> {
        // Always handle a higher term first.
        if msg.term > self.persistent.current_term {
            self.step_down(msg.term);
        }

        // Candidate is stale.
        if msg.term < self.persistent.current_term {
            return vec![self.vote_response(msg.candidate_id, false)];
        }

        // Already voted for a different candidate this term.
        if let Some(voted_for) = self.persistent.voted_for
            && voted_for != msg.candidate_id
        {
            return vec![self.vote_response(msg.candidate_id, false)];
        }

        // Candidate log must be at least as up-to-date as ours.
        if !self.candidate_log_up_to_date(&msg) {
            return vec![self.vote_response(msg.candidate_id, false)];
        }

        // Grant vote.
        self.persistent.voted_for = Some(msg.candidate_id);
        vec![
            Action::ResetElectionTimer,
            self.vote_response(msg.candidate_id, true),
        ]
    }

    pub(crate) fn handle_vote_response(&mut self, msg: RequestVoteResponse) -> Vec<Action> {
        // Higher term: step down.
        if msg.term > self.persistent.current_term {
            self.step_down(msg.term);
            return vec![];
        }

        // Stale response.
        if msg.term < self.persistent.current_term {
            return vec![];
        }

        // Only candidates process vote responses.
        if self.role != Role::Candidate {
            return vec![];
        }

        // Reject votes from unknown peers.
        if !self.peers.contains(&msg.vote_from) {
            return vec![];
        }

        let quorum = self.quorum();

        let state = self.candidate_state.get_or_insert_with(Default::default);

        // Prevent multiple votes from same peer.
        if !state.votes_received.insert(msg.vote_from) {
            return vec![];
        }

        if msg.vote_granted {
            state.votes_granted.insert(msg.vote_from);
        }

        // if not yet won, continue waiting.
        if !state.has_majority(quorum) {
            return vec![];
        }

        // we won: become leader and initialize statistics.
        self.become_leader();

        // Note: do not reset election timer.
        vec![Action::ResetHeartbeatTimer]
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    pub(crate) fn step_down(&mut self, new_term: Term) {
        self.persistent.current_term = new_term;
        self.role = Role::Follower;
        self.persistent.voted_for = None;
        self.candidate_state = None;
        self.leader_state = None;
        self.known_leader = None;
    }

    /// Returns true if the candidate's log is at least as up-to-date as ours.
    fn candidate_log_up_to_date(&self, msg: &RequestVote) -> bool {
        let last_term = self.last_log_term();
        let last_index = self.last_log_index();

        msg.last_log_term > last_term
            || (msg.last_log_term == last_term && msg.last_log_index >= last_index)
    }

    fn vote_response(&self, candidate_id: NodeId, granted: bool) -> Action {
        Action::Send(
            candidate_id,
            Message::RequestVoteResponse(RequestVoteResponse {
                term: self.persistent.current_term,
                vote_granted: granted,
                vote_from: self.id,
            }),
        )
    }

    pub(crate) fn become_leader(&mut self) {
        debug_assert_eq!(
            self.role,
            Role::Candidate,
            "become_leader called outside candidacy"
        );

        let next = self.last_log_index() + 1;
        self.role = Role::Leader;
        self.known_leader = Some(self.id);
        self.candidate_state = None;
        self.leader_state = Some(LeaderState {
            next_index: vec![next; self.peers.len()],
            match_index: vec![0; self.peers.len()],
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        CandidateState, Raft, Role,
        protocol::{
            action::Action,
            command::Command,
            log::LogEntry,
            message::{Message, RequestVote, RequestVoteResponse},
            types::NodeId,
        },
    };

    #[test]
    fn reject_stale_candidate_term() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.current_term = 5;

        let actions = raft.handle_request_vote(vote_request(4, 2, 0, 0));

        assert!(!grant(&actions));
        assert_eq!(response_term(&actions), 5);
        assert!(!has_reset_timer(&actions));
    }

    #[test]
    fn higher_term_causes_step_down_before_vote() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.current_term = 2;
        raft.role = Role::Leader;
        raft.persistent.voted_for = Some(3);

        let _ = raft.handle_request_vote(vote_request(5, 2, 0, 0));

        assert_eq!(raft.persistent.current_term, 5);
        assert_eq!(raft.role, Role::Follower);
        assert_eq!(raft.persistent.voted_for, Some(2));
    }

    #[test]
    fn higher_term_vote_response_triggers_step_down_before_other_checks() {
        // Previously the peer-unknown check was before the term check;
        // this test ensures step-down fires even for unknown peers.
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Candidate;
        raft.persistent.current_term = 5;

        let msg = RequestVoteResponse {
            term: 9,
            vote_granted: true,
            vote_from: 99,
        };
        let actions = raft.handle_vote_response(msg);

        assert_eq!(raft.role, Role::Follower);
        assert_eq!(raft.persistent.current_term, 9);
        assert!(actions.is_empty());
    }

    #[test]
    fn reject_if_already_voted_for_different_candidate() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.current_term = 3;
        raft.persistent.voted_for = Some(10);

        let actions = raft.handle_request_vote(vote_request(3, 2, 0, 0));

        assert!(!grant(&actions));
        assert!(!has_reset_timer(&actions));
    }

    #[test]
    fn allow_same_candidate_to_retry_vote() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.current_term = 3;
        raft.persistent.voted_for = Some(2);

        let actions = raft.handle_request_vote(vote_request(3, 2, 0, 0));

        assert!(grant(&actions));
        assert!(has_reset_timer(&actions));
    }

    #[test]
    fn reject_if_candidate_log_term_is_older() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 5, 3);

        let actions = raft.handle_request_vote(vote_request(4, 2, 10, 2));
        assert!(!grant(&actions));
    }

    #[test]
    fn reject_if_same_log_term_but_shorter_log() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 10, 3);

        let actions = raft.handle_request_vote(vote_request(4, 2, 5, 3));
        assert!(!grant(&actions));
    }

    #[test]
    fn accept_if_candidate_log_term_is_newer() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 10, 2);

        let actions = raft.handle_request_vote(vote_request(3, 2, 1, 5));
        assert!(grant(&actions));
    }

    #[test]
    fn accept_if_same_log_term_and_longer_log() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 5, 3);

        let actions = raft.handle_request_vote(vote_request(4, 2, 10, 3));
        assert!(grant(&actions));
    }

    #[test]
    fn accept_if_logs_identical() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 5, 3);

        let actions = raft.handle_request_vote(vote_request(4, 2, 5, 3));
        assert!(grant(&actions));
    }

    #[test]
    fn accept_if_follower_log_empty() {
        let mut raft = new_raft(1, &[2, 3]);

        let actions = raft.handle_request_vote(vote_request(1, 2, 0, 0));
        assert!(grant(&actions));
    }

    #[test]
    fn log_term_takes_priority_over_log_length() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 100, 9);

        // candidate has shorter log but newer term → should win
        let actions = raft.handle_request_vote(vote_request(11, 2, 5, 10));
        assert!(grant(&actions));
    }

    #[test]
    fn reject_if_same_index_but_older_term() {
        let mut raft = new_raft(1, &[2, 3]);
        push_entry(&mut raft, 10, 5);

        let actions = raft.handle_request_vote(vote_request(6, 2, 10, 4));
        assert!(!grant(&actions));
    }

    #[test]
    fn never_vote_twice_in_same_term() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.current_term = 4;

        let a1 = raft.handle_request_vote(vote_request(4, 2, 0, 0));
        let a2 = raft.handle_request_vote(vote_request(4, 3, 0, 0));

        assert!(grant(&a1));
        assert!(!grant(&a2));
    }

    #[test]
    fn higher_term_clears_previous_vote() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.current_term = 3;
        raft.persistent.voted_for = Some(10);

        let actions = raft.handle_request_vote(vote_request(4, 2, 0, 0));

        assert!(grant(&actions));
        assert_eq!(raft.persistent.current_term, 4);
        assert_eq!(raft.persistent.voted_for, Some(2));
    }

    #[test]
    fn timer_reset_only_on_vote_granted() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.voted_for = Some(9);

        let actions = raft.handle_request_vote(vote_request(0, 2, 0, 0));
        assert!(!has_reset_timer(&actions));
    }

    // ── handle_vote_response ─────────────────────────────────────────────────

    #[test]
    fn ignore_stale_vote_response() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.persistent.current_term = 5;

        let actions = raft.handle_vote_response(vote_resp(4, 1, true));
        assert!(actions.is_empty());
    }

    #[test]
    fn ignore_vote_response_when_not_candidate() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Follower;

        let actions = raft.handle_vote_response(vote_resp(0, 2, true));
        assert!(actions.is_empty());
    }

    #[test]
    fn ignore_vote_from_unknown_peer() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Candidate;

        let actions = raft.handle_vote_response(vote_resp(0, 99, true));
        assert!(actions.is_empty());
        assert_eq!(raft.role, Role::Candidate);
    }

    #[test]
    fn become_leader_on_majority() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Candidate;
        raft.persistent.current_term = 1;

        // self-vote already in candidate_state from election_timeout,
        // simulate that here.
        let mut cs = CandidateState::default();
        cs.votes_received.insert(1);
        cs.votes_granted.insert(1);
        raft.candidate_state = Some(cs);

        // peer 2 grants → 2 of 3 = majority
        let actions = raft.handle_vote_response(vote_resp(1, 2, true));

        assert_eq!(raft.role, Role::Leader);
        assert!(matches!(actions[0], Action::ResetHeartbeatTimer));
    }

    #[test]
    fn continue_waiting_if_majority_not_reached() {
        let mut raft = new_raft(1, &[2, 3, 4, 5]);
        raft.role = Role::Candidate;
        raft.persistent.current_term = 3;

        let mut cs = CandidateState::default();
        cs.votes_received.insert(1);
        cs.votes_granted.insert(1); // only self so far; quorum = 3
        raft.candidate_state = Some(cs);

        let actions = raft.handle_vote_response(vote_resp(3, 2, true));

        assert_eq!(raft.role, Role::Candidate);
        assert!(actions.is_empty());
    }

    #[test]
    fn step_down_on_higher_term_vote_response() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Candidate;
        raft.persistent.current_term = 5;

        let actions = raft.handle_vote_response(vote_resp(6, 2, true));

        assert_eq!(raft.role, Role::Follower);
        assert_eq!(raft.persistent.current_term, 6);
        assert!(raft.persistent.voted_for.is_none());
        assert!(raft.candidate_state.is_none());
        assert!(actions.is_empty());
    }

    #[test]
    fn duplicate_vote_response_is_ignored() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Candidate;

        let mut cs = CandidateState::default();
        cs.votes_received.insert(1);
        cs.votes_granted.insert(1);
        cs.votes_received.insert(2); // peer 2 already seen
        raft.candidate_state = Some(cs);

        let actions = raft.handle_vote_response(vote_resp(0, 2, true));
        assert!(actions.is_empty());
        assert_eq!(
            raft.candidate_state.as_ref().unwrap().votes_granted.len(),
            1
        );
    }

    #[test]
    fn rejected_vote_does_not_increase_granted_count() {
        let mut raft = new_raft(1, &[2, 3, 4, 5]);
        raft.role = Role::Candidate;

        let mut cs = CandidateState::default();
        cs.votes_received.insert(1);
        cs.votes_granted.insert(1);
        raft.candidate_state = Some(cs);

        raft.handle_vote_response(vote_resp(0, 2, false));

        let cs = raft.candidate_state.as_ref().unwrap();
        assert_eq!(cs.votes_granted.len(), 1);
    }

    #[test]
    fn leader_transition_happens_only_once() {
        let mut raft = new_raft(1, &[2, 3, 4]);
        raft.role = Role::Candidate;
        raft.persistent.current_term = 1;

        let mut cs = CandidateState::default();
        cs.votes_received.extend([1, 2]);
        cs.votes_granted.extend([1, 2]);
        raft.candidate_state = Some(cs);

        raft.handle_vote_response(vote_resp(1, 3, true)); // becomes leader
        assert_eq!(raft.role, Role::Leader);

        // Further response must be ignored (role != Candidate).
        let actions = raft.handle_vote_response(vote_resp(1, 4, true));
        assert!(actions.is_empty());
    }

    #[test]
    fn leader_state_initialized_correctly_on_win() {
        let mut raft = new_raft(1, &[2, 3]);
        raft.role = Role::Candidate;
        raft.persistent.current_term = 1;

        let mut cs = CandidateState::default();
        cs.votes_received.insert(1);
        cs.votes_granted.insert(1);
        raft.candidate_state = Some(cs);

        raft.handle_vote_response(vote_resp(1, 2, true));

        let ls = raft.leader_state.as_ref().unwrap();
        let expected_next = raft.last_log_index() + 1;
        assert_eq!(ls.next_index, vec![expected_next; raft.peers.len()]);
        assert_eq!(ls.match_index, vec![0; raft.peers.len()]);
    }

    #[test]
    fn quorum_correct_for_odd_and_even_clusters() {
        assert_eq!(new_raft(1, &[2, 3, 4, 5]).quorum(), 3); // 5-node
        assert_eq!(new_raft(1, &[2, 3, 4]).quorum(), 3); // 4-node (conservative)
        assert_eq!(new_raft(1, &[2, 3]).quorum(), 2); // 3-node
        assert_eq!(new_raft(1, &[2]).quorum(), 2); // 2-node
        assert_eq!(new_raft(1, &[]).quorum(), 1); // 1-node
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn new_raft(id: NodeId, peers: &[NodeId]) -> Raft {
        Raft::new(id, peers.to_vec())
    }

    fn vote_request(term: u64, candidate: NodeId, last_index: u64, last_term: u64) -> RequestVote {
        RequestVote {
            term,
            candidate_id: candidate,
            last_log_index: last_index,
            last_log_term: last_term,
        }
    }

    fn vote_resp(term: u64, from: NodeId, granted: bool) -> RequestVoteResponse {
        RequestVoteResponse {
            term,
            vote_granted: granted,
            vote_from: from,
        }
    }

    fn push_entry(raft: &mut Raft, index: u64, term: u64) {
        raft.persistent.log.push(LogEntry {
            index,
            term,
            command: Command::default(),
        });
    }

    fn grant(actions: &[Action]) -> bool {
        actions.iter().any(
            |a| matches!(a, Action::Send(_, Message::RequestVoteResponse(v)) if v.vote_granted),
        )
    }

    fn response_term(actions: &[Action]) -> u64 {
        actions
            .iter()
            .find_map(|a| match a {
                Action::Send(_, Message::RequestVoteResponse(v)) => Some(v.term),
                _ => None,
            })
            .expect("no vote response")
    }

    fn has_reset_timer(actions: &[Action]) -> bool {
        actions
            .iter()
            .any(|a| matches!(a, Action::ResetElectionTimer))
    }
}
