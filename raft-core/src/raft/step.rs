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
