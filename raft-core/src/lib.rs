use crate::protocol::{
    log::LogEntry,
    types::{LogIndex, NodeId, Term},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub mod protocol;
pub mod raft;

// ── Role ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

// ── State structs ─────────────────────────────────────────────────────────────

/// Persistent state — must be saved to stable storage before responding to RPCs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistentState {
    /// latest term server has seen
    pub current_term: Term,
    /// candidateId that received vote in current term (or null if none)
    pub voted_for: Option<NodeId>,
    /// each entry contains command for statemachine, and term when entry
    /// was received by leader (first index is 1)
    pub log: Vec<LogEntry>,
}
/// Volatile election state — only valid while role == Candidate.
#[derive(Debug, Clone, Default)]
pub struct CandidateState {
    /// All peers from which a response (grant or reject) has been received.
    pub votes_received: HashSet<NodeId>,
    /// Peers that granted their vote.
    pub votes_granted: HashSet<NodeId>,
}

impl CandidateState {
    /// Returns true if the number of granted votes meets or exceeds `quorum`.
    pub fn has_majority(&self, quorum: usize) -> bool {
        self.votes_granted.len() >= quorum
    }
}

/// Volatile state — reset on every startup.
#[derive(Debug, Clone, Default)]
pub struct VolatileState {
    /// index of highest log entry known to be committed
    pub commit_index: LogIndex,
    /// index of highest log entry applied to state machine
    pub last_applied: LogIndex,
}

/// Leader-only volatile state — reset whenever a new leader is elected.
#[derive(Debug, Clone)]
pub struct LeaderState {
    /// next_index[i] — next log index to send to peers[i]
    pub next_index: Vec<LogIndex>,
    /// match_index[i] — highest log index known replicated on peers[i]
    pub match_index: Vec<LogIndex>,
}

// ── Raft ──────────────────────────────────────────────────────────────────────

/// Core Raft state machine.
///
/// `peers` excludes `self.id`.  All indexing into `leader_state` arrays
/// uses the position of the peer in `self.peers`.
pub struct Raft {
    pub id: NodeId,
    pub peers: Vec<NodeId>,
    pub role: Role,
    pub known_leader: Option<NodeId>,
    pub persistent: PersistentState,
    pub volatile: VolatileState,
    pub leader_state: Option<LeaderState>,
    pub candidate_state: Option<CandidateState>,
}

impl Raft {
    pub fn new(id: NodeId, peers: Vec<NodeId>) -> Self {
        debug_assert!(
            !peers.contains(&id),
            "peers must not contain self (id={id})"
        );
        Self {
            id,
            peers,
            role: Role::Follower,
            known_leader: None,
            persistent: PersistentState::default(),
            volatile: VolatileState::default(),
            leader_state: None,
            candidate_state: None,
        }
    }

    /// Restore from persisted state after a crash.
    pub fn restore(id: NodeId, peers: Vec<NodeId>, persistent: PersistentState) -> Self {
        let mut r = Self::new(id, peers);
        r.persistent = persistent;
        r
    }
    /// Quorum size: how many votes (including self) are needed to win.
    ///
    /// With N peers (excluding self), cluster size = N+1, majority = ⌊(N+1)/2⌋ + 1.
    pub fn quorum(&self) -> usize {
        self.peers.len().div_ceil(2) + 1
    }

    /// Index of the last log entry, or 0 if the log is empty.
    pub fn last_log_index(&self) -> LogIndex {
        self.persistent.log.last().map_or(0, |e| e.index)
    }

    /// Term of the last log entry, or 0 if the log is empty.
    pub fn last_log_term(&self) -> Term {
        self.persistent.log.last().map_or(0, |e| e.term)
    }
}
