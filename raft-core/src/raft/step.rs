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
    use super::*;
    use crate::protocol::{
        command::Command,
        event::Event,
        message::{
            AppendEntries, AppendEntriesResponse, Message, RequestVote, RequestVoteResponse,
        },
    };

    /// Helper to create a Raft node
    fn new_node() -> Raft {
        Raft::new(1, vec![2, 3])
    }

    #[test]
    fn step_handles_election_timeout() {
        let mut raft = new_node();
        let actions = raft.step(Event::ElectionTimeout);

        // Count ResetElectionTimer actions
        let reset_timer_count = actions
            .iter()
            .filter(|a| matches!(a, Action::ResetElectionTimer))
            .count();
        assert_eq!(
            reset_timer_count, 1,
            "should have exactly 1 ResetElectionTimer"
        );

        // Count RequestVote messages
        let request_vote_count = actions
            .iter()
            .filter(|a| matches!(a, Action::Send(_, Message::RequestVote(_))))
            .count();
        assert_eq!(
            request_vote_count, 2,
            "should have exactly 2 RequestVote messages"
        );

        // Optional: check total actions
        assert_eq!(actions.len(), 3, "total actions should be 3");
    }

    #[test]
    fn step_handles_heartbeat_timeout() {
        let mut raft = new_node();
        let actions = raft.step(Event::HeartbeatTimeout);
        assert!(actions.is_empty());
    }

    #[test]
    fn step_handles_client_request() {
        let mut raft = new_node();
        let actions = raft.step(Event::ClientRequest(Command::default()));
        assert!(actions.is_empty());
    }

    #[test]
    fn step_handles_message_request_vote() {
        let mut raft = new_node();
        let msg = Message::RequestVote(RequestVote::default());
        let actions = raft.step(Event::Message(msg));
        assert!(actions.is_empty());
    }

    #[test]
    fn step_handles_message_request_vote_response() {
        let mut raft = new_node();
        let msg = Message::RequestVoteResponse(RequestVoteResponse::default());
        let actions = raft.step(Event::Message(msg));
        assert!(actions.is_empty());
    }

    #[test]
    fn step_handles_message_append_entries() {
        let mut raft = new_node();
        let msg = Message::AppendEntries(AppendEntries::default());
        let actions = raft.step(Event::Message(msg));
        assert!(actions.is_empty());
    }

    #[test]
    fn step_handles_message_append_entries_response() {
        let mut raft = new_node();
        let msg = Message::AppendEntriesResponse(AppendEntriesResponse::default());
        let actions = raft.step(Event::Message(msg));
        assert!(actions.is_empty());
    }
}
