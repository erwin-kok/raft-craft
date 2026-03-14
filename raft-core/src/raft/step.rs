use crate::{
    Raft,
    protocol::{action::Action, event::Event, message::Message},
};

impl Raft {
    /// Step the Raft state machine with an incoming event
    /// Returns a list of Actions that should be executed (send messages, timers, etc.)
    pub fn step(&mut self, event: Event) -> Vec<Action> {
        match event {
            Event::Message(msg) => self.handle_message(msg),
            Event::ElectionTimeout => self.handle_election_timeout(),
            Event::HeartbeatTimeout => self.handle_heartbeat_timeout(),
            Event::ClientRequest(cmd) => self.handle_client_request(cmd),
        }
    }

    fn handle_message(&mut self, msg: Message) -> Vec<Action> {
        match msg {
            Message::RequestVote(m) => self.handle_request_vote(m),
            Message::RequestVoteResponse(m) => self.handle_vote_response(m),
            Message::AppendEntries(m) => self.handle_append_entries(m),
            Message::AppendEntriesResponse(m) => self.handle_append_entries_response(m),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        CandidateState, Raft, Role,
        protocol::{
            action::Action,
            command::Command,
            event::Event,
            message::{
                AppendEntries, AppendEntriesResponse, Message, RequestVote, RequestVoteResponse,
            },
            types::NodeId,
        },
    };

    // ════════════════════════════════════════════════════════════════════════
    // ElectionTimeout
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn election_timeout_as_follower_starts_election() {
        let mut raft = new_follower();

        let actions = raft.step(Event::ElectionTimeout);

        // Exactly one timer reset and one RequestVote per peer.
        assert_eq!(
            count(&actions, |a| matches!(a, Action::ResetElectionTimer)),
            1
        );
        assert_eq!(
            count(&actions, |a| matches!(
                a,
                Action::Send(_, Message::RequestVote(_))
            )),
            2,
        );
        assert_eq!(actions.len(), 3);

        // State transitions.
        assert_eq!(raft.role, Role::Candidate);
        assert_eq!(raft.persistent.current_term, 1);
        assert_eq!(raft.persistent.voted_for, Some(raft.id));
    }

    #[test]
    fn election_timeout_vote_requests_target_correct_peers() {
        let mut raft = new_follower();

        let actions = raft.step(Event::ElectionTimeout);

        let targets: Vec<NodeId> = actions
            .iter()
            .filter_map(|a| {
                if let Action::Send(id, Message::RequestVote(_)) = a {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();

        assert!(targets.contains(&2));
        assert!(targets.contains(&3));
        assert!(!targets.contains(&1), "must not send to self");
    }

    #[test]
    fn election_timeout_vote_requests_carry_correct_term() {
        let mut raft = new_follower();

        let actions = raft.step(Event::ElectionTimeout);

        for a in &actions {
            if let Action::Send(_, Message::RequestVote(v)) = a {
                assert_eq!(v.term, 1, "vote request must carry incremented term");
                assert_eq!(v.candidate_id, 1);
            }
        }
    }

    #[test]
    fn election_timeout_as_candidate_increments_term_again() {
        let mut raft = new_follower();
        // First timeout → candidate, term=1.
        raft.step(Event::ElectionTimeout);
        // Second timeout → still candidate, term=2.
        let actions = raft.step(Event::ElectionTimeout);

        assert_eq!(raft.persistent.current_term, 2);
        assert_eq!(raft.role, Role::Candidate);
        assert_eq!(
            count(&actions, |a| matches!(a, Action::ResetElectionTimer)),
            1
        );
        assert_eq!(
            count(&actions, |a| matches!(
                a,
                Action::Send(_, Message::RequestVote(_))
            )),
            2,
        );
    }

    #[test]
    fn election_timeout_as_leader_is_ignored() {
        let mut raft = make_leader();

        let actions = raft.step(Event::ElectionTimeout);

        assert!(actions.is_empty());
        assert_eq!(raft.role, Role::Leader);
        assert_eq!(raft.persistent.current_term, 1);
    }

    // ════════════════════════════════════════════════════════════════════════
    // HeartbeatTimeout
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn heartbeat_timeout_as_follower_is_ignored() {
        let mut raft = new_follower();

        let actions = raft.step(Event::HeartbeatTimeout);

        assert!(actions.is_empty());
    }

    #[test]
    fn heartbeat_timeout_as_candidate_is_ignored() {
        let mut raft = new_follower();
        raft.step(Event::ElectionTimeout); // become candidate

        let actions = raft.step(Event::HeartbeatTimeout);

        assert!(actions.is_empty());
    }

    #[test]
    fn heartbeat_timeout_as_leader_sends_append_entries_to_all_peers() {
        let mut raft = make_leader();

        let actions = raft.step(Event::HeartbeatTimeout);

        assert_eq!(
            count(&actions, |a| matches!(a, Action::ResetHeartbeatTimer)),
            1
        );
        assert_eq!(
            count(&actions, |a| matches!(
                a,
                Action::Send(_, Message::AppendEntries(_))
            )),
            2,
        );
        assert_eq!(actions.len(), 3);
    }

    #[test]
    fn heartbeat_timeout_append_entries_carry_correct_term_and_leader_id() {
        let mut raft = make_leader();

        let actions = raft.step(Event::HeartbeatTimeout);

        for a in &actions {
            if let Action::Send(_, Message::AppendEntries(m)) = a {
                assert_eq!(m.term, 1);
                assert_eq!(m.leader_id, 1);
            }
        }
    }

    #[test]
    fn heartbeat_timeout_append_entries_target_correct_peers() {
        let mut raft = make_leader();

        let actions = raft.step(Event::HeartbeatTimeout);

        let targets: Vec<NodeId> = actions
            .iter()
            .filter_map(|a| {
                if let Action::Send(id, Message::AppendEntries(_)) = a {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();

        assert!(targets.contains(&2));
        assert!(targets.contains(&3));
        assert!(!targets.contains(&1));
    }

    // ════════════════════════════════════════════════════════════════════════
    // ClientRequest
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn client_request_as_follower_returns_not_leader() {
        let mut raft = new_follower();

        let actions = raft.step(Event::ClientRequest(Command::default()));

        assert_eq!(count(&actions, |a| matches!(a, Action::NotLeader(_))), 1);
        assert_eq!(actions.len(), 1);
        assert!(raft.persistent.log.is_empty());
    }

    #[test]
    fn client_request_as_candidate_returns_not_leader() {
        let mut raft = new_follower();
        raft.step(Event::ElectionTimeout); // become candidate

        let actions = raft.step(Event::ClientRequest(Command::default()));

        assert!(matches!(actions[..], [Action::NotLeader(_)]));
        assert!(raft.persistent.log.is_empty());
    }

    #[test]
    fn client_request_as_leader_appends_and_triggers_heartbeat() {
        let mut raft = make_leader();

        let actions = raft.step(Event::ClientRequest(Command::default()));

        assert_eq!(
            count(&actions, |a| matches!(a, Action::ResetHeartbeatTimer)),
            1
        );
        assert_eq!(actions.len(), 1);
        assert_eq!(raft.persistent.log.len(), 1);
        assert_eq!(raft.persistent.log[0].index, 1);
        assert_eq!(raft.persistent.log[0].term, 1);
    }

    #[test]
    fn client_request_as_leader_does_not_commit_without_quorum() {
        let mut raft = make_leader();

        raft.step(Event::ClientRequest(Command::default()));

        assert_eq!(raft.volatile.commit_index, 0);
    }

    // ════════════════════════════════════════════════════════════════════════
    // Message::RequestVote
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn request_vote_from_peer_with_same_term_is_granted() {
        let mut raft = new_follower(); // current_term = 0

        // Use a real peer id and matching term so the vote is granted cleanly.
        let msg = Message::RequestVote(RequestVote {
            term: 0,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        });
        let actions = raft.step(Event::Message(msg));

        assert_eq!(
            count(&actions, |a| matches!(a, Action::ResetElectionTimer)),
            1
        );
        assert_eq!(
            count(&actions, |a| matches!(
                a,
                Action::Send(_, Message::RequestVoteResponse(_))
            )),
            1,
        );
        assert_eq!(actions.len(), 2);

        // Response must be addressed to the candidate and grant the vote.
        let resp = find_vote_response(&actions);
        assert!(resp.vote_granted);
        assert_eq!(resp.term, 0);
    }

    #[test]
    fn request_vote_from_higher_term_steps_down_and_grants() {
        let mut raft = make_leader(); // term=1, role=Leader

        let msg = Message::RequestVote(RequestVote {
            term: 3,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        });
        raft.step(Event::Message(msg));

        assert_eq!(raft.role, Role::Follower);
        assert_eq!(raft.persistent.current_term, 3);
    }

    #[test]
    fn request_vote_from_stale_term_is_rejected() {
        let mut raft = new_follower();
        raft.persistent.current_term = 5;

        let msg = Message::RequestVote(RequestVote {
            term: 3,
            candidate_id: 2,
            last_log_index: 0,
            last_log_term: 0,
        });
        let actions = raft.step(Event::Message(msg));

        let resp = find_vote_response(&actions);
        assert!(!resp.vote_granted);
        assert_eq!(resp.term, 5);
        assert_eq!(
            count(&actions, |a| matches!(a, Action::ResetElectionTimer)),
            0
        );
    }

    #[test]
    fn request_vote_response_as_follower_is_ignored() {
        let mut raft = new_follower();

        let msg = Message::RequestVoteResponse(RequestVoteResponse {
            term: 0,
            vote_granted: true,
            vote_from: 2,
        });
        let actions = raft.step(Event::Message(msg));

        assert!(actions.is_empty());
        assert_eq!(raft.role, Role::Follower);
    }

    #[test]
    fn request_vote_response_grants_cause_leader_promotion() {
        // Node 1 in a 3-node cluster needs 2 votes (self + 1 peer).
        // Simulate: already voted for self (from ElectionTimeout), now peer 2 grants.
        let mut raft = new_follower();
        raft.step(Event::ElectionTimeout); // term=1, candidate, self-vote counted

        let msg = Message::RequestVoteResponse(RequestVoteResponse {
            term: 1,
            vote_granted: true,
            vote_from: 2,
        });
        let actions = raft.step(Event::Message(msg));

        assert_eq!(raft.role, Role::Leader);
        assert_eq!(
            count(&actions, |a| matches!(a, Action::ResetHeartbeatTimer)),
            1
        );
    }

    // ════════════════════════════════════════════════════════════════════════
    // Message::AppendEntries
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn append_entries_with_matching_term_accepted_and_resets_timer() {
        let mut raft = new_follower();

        let msg = Message::AppendEntries(AppendEntries {
            term: 0,
            leader_id: 2,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        });
        let actions = raft.step(Event::Message(msg));

        assert_eq!(
            count(&actions, |a| matches!(a, Action::ResetElectionTimer)),
            1
        );
        assert_eq!(
            count(&actions, |a| matches!(
                a,
                Action::Send(_, Message::AppendEntriesResponse(_))
            )),
            1,
        );
        assert_eq!(actions.len(), 2);

        let resp = find_append_entries_response(&actions);
        assert!(resp.success);
        assert_eq!(resp.term, 0);
    }

    #[test]
    fn append_entries_with_stale_term_is_rejected() {
        let mut raft = new_follower();
        raft.persistent.current_term = 5;

        let msg = Message::AppendEntries(AppendEntries {
            term: 3,
            leader_id: 2,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        });
        let actions = raft.step(Event::Message(msg));

        let resp = find_append_entries_response(&actions);
        assert!(!resp.success);
        assert_eq!(resp.term, 5);
        assert_eq!(
            count(&actions, |a| matches!(a, Action::ResetElectionTimer)),
            0
        );
    }

    #[test]
    fn append_entries_from_higher_term_steps_down_and_accepts() {
        let mut raft = make_leader(); // role=Leader, term=1

        let msg = Message::AppendEntries(AppendEntries {
            term: 5,
            leader_id: 2,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        });
        raft.step(Event::Message(msg));

        assert_eq!(raft.role, Role::Follower);
        assert_eq!(raft.persistent.current_term, 5);
    }

    #[test]
    fn append_entries_response_as_follower_is_ignored() {
        let mut raft = new_follower();

        let msg = Message::AppendEntriesResponse(AppendEntriesResponse {
            term: 0,
            success: true,
            from: 2,
            match_index: 0,
        });
        let actions = raft.step(Event::Message(msg));

        assert!(actions.is_empty());
    }

    #[test]
    fn append_entries_response_success_advances_match_index_on_leader() {
        let mut raft = make_leader();
        // Give the leader a log entry so there's something to replicate.
        raft.step(Event::ClientRequest(Command::default()));

        let msg = Message::AppendEntriesResponse(AppendEntriesResponse {
            term: 1,
            success: true,
            from: 2,
            match_index: 1,
        });
        raft.step(Event::Message(msg));

        let ls = raft.leader_state.as_ref().unwrap();
        // Peer 2 is at index 0 in peers=[2,3].
        assert_eq!(ls.match_index[0], 1);
        assert_eq!(ls.next_index[0], 2);
    }

    #[test]
    fn append_entries_response_reaching_quorum_commits_and_applies() {
        let mut raft = make_leader();
        raft.step(Event::ClientRequest(Command::default()));

        // Peer 2 confirms index 1 — that's quorum (leader + peer 2 = 2 of 3).
        let actions = raft.step(Event::Message(Message::AppendEntriesResponse(
            AppendEntriesResponse {
                term: 1,
                success: true,
                from: 2,
                match_index: 1,
            },
        )));

        assert_eq!(raft.volatile.commit_index, 1);
        assert_eq!(raft.volatile.last_applied, 1);
        assert_eq!(count(&actions, |a| matches!(a, Action::ApplyCommand(_))), 1);
    }

    // ════════════════════════════════════════════════════════════════════════
    // helpers
    // ════════════════════════════════════════════════════════════════════════

    /// Follower in a 3-node cluster (peers 2 and 3), term 0.
    fn new_follower() -> Raft {
        Raft::new(1, vec![2, 3])
    }

    /// Leader in the same 3-node cluster, term 1, with fresh leader state.
    fn make_leader() -> Raft {
        let mut raft = new_follower();
        // Simulate a completed election: node is candidate having voted for itself.
        // Only the self-vote is pre-seeded; the winning grant arrives via step()
        // below so the deduplication check in handle_vote_response is not tripped.
        raft.role = Role::Candidate;
        raft.persistent.current_term = 1;
        raft.persistent.voted_for = Some(1);
        let mut cs = CandidateState::default();
        cs.votes_received.insert(1); // self
        cs.votes_granted.insert(1); // self
        raft.candidate_state = Some(cs);
        // Peer 2 grants — not yet in votes_received, so deduplication passes
        // and this tips us over quorum (self + peer 2 = 2 of 3).
        raft.step(Event::Message(Message::RequestVoteResponse(
            RequestVoteResponse {
                term: 1,
                vote_granted: true,
                vote_from: 2,
            },
        )));
        assert_eq!(raft.role, Role::Leader, "make_leader setup failed");
        raft
    }

    fn count<F>(actions: &[Action], f: F) -> usize
    where
        F: Fn(&Action) -> bool,
    {
        actions.iter().filter(|a| f(a)).count()
    }

    fn find_vote_response(actions: &[Action]) -> &RequestVoteResponse {
        actions
            .iter()
            .find_map(|a| match a {
                Action::Send(_, Message::RequestVoteResponse(r)) => Some(r),
                _ => None,
            })
            .expect("no RequestVoteResponse found")
    }

    fn find_append_entries_response(actions: &[Action]) -> &AppendEntriesResponse {
        actions
            .iter()
            .find_map(|a| match a {
                Action::Send(_, Message::AppendEntriesResponse(r)) => Some(r),
                _ => None,
            })
            .expect("no AppendEntriesResponse found")
    }
}
