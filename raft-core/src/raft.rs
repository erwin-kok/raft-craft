use crate::{
    action::Action,
    command::Command,
    event::Event,
    log::LogEntry,
    message::{AppendEntries, AppendEntriesResponse, Message, RequestVote, RequestVoteResponse},
    types::{LogIndex, NodeId, Term},
};

/// Raft role
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

/// Raft persistent state
#[derive(Debug, Clone)]
pub struct PersistentState {
    pub current_term: Term,
    pub voted_for: Option<NodeId>,
    pub log: Vec<LogEntry>,
}

/// Raft volatile state
#[derive(Debug, Clone)]
pub struct VolatileState {
    pub commit_index: LogIndex,
    pub last_applied: LogIndex,
}

/// Raft leader volatile state (nextIndex and matchIndex per follower)
#[derive(Debug, Clone)]
pub struct LeaderState {
    pub next_index: Vec<LogIndex>,
    pub match_index: Vec<LogIndex>,
}

/// Raft core struct
pub struct Raft {
    pub id: NodeId,
    pub peers: Vec<NodeId>,
    pub role: Role,
    pub persistent: PersistentState,
    pub volatile: VolatileState,
    pub leader_state: Option<LeaderState>,
}

impl Raft {
    /// Create a new Raft node
    pub fn new(id: NodeId, peers: Vec<NodeId>) -> Self {
        Self {
            id,
            peers,
            role: Role::Follower,
            persistent: PersistentState {
                current_term: 0,
                voted_for: None,
                log: Vec::new(),
            },
            volatile: VolatileState {
                commit_index: 0,
                last_applied: 0,
            },
            leader_state: None,
        }
    }
    
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

    fn handle_election_timeout(&mut self) -> Vec<Action> {
        vec![]
    }

    fn handle_heartbeat_timeout(&mut self) -> Vec<Action> {
        vec![]
    }

    fn handle_client_request(&mut self, _command: Command) -> Vec<Action> {
        vec![]
    }

    fn handle_request_vote(&mut self, _msg: RequestVote) -> Vec<Action> {
        vec![]
    }

    fn handle_vote_response(&mut self, _msg: RequestVoteResponse) -> Vec<Action> {
        vec![]
    }

    fn handle_append_entries(&mut self, _msg: AppendEntries) -> Vec<Action> {
        vec![]
    }

    fn handle_append_entries_response(&mut self, _msg: AppendEntriesResponse) -> Vec<Action> {
        vec![]
    }
}
