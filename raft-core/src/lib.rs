use crate::protocol::{
    log::LogEntry,
    types::{LogIndex, NodeId, Term},
};

mod protocol;
mod raft;

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
}
