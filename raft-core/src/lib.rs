use crate::protocol::{
    log::LogEntry,
    types::{LogIndex, NodeId, Term},
};
use std::collections::HashSet;

mod protocol;
mod raft;

/// Raft role
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

/// Raft persistent state (must survive crashes)
///
/// Note: `voted_for` is term-scoped; must be reset when `current_term` advances.
#[derive(Debug, Default, Clone)]
pub struct PersistentState {
    /// latest term server has seen
    pub(crate) current_term: Term,
    /// candidateId that received vote in current term (or null if none)
    pub(crate) voted_for: Option<NodeId>,
    /// each entry contains command for statemachine, and term when entry
    /// was received by leader (first index is 1)
    pub(crate) log: Vec<LogEntry>,
}

/// Volatile election state, only valid while role == Candidate.
/// Cleared on every role transition.
#[derive(Debug, Clone, Default)]
pub struct CandidateState {
    /// All peers from which a response (grant or reject) has been received.
    pub(crate) votes_received: HashSet<NodeId>,
    /// Peers that granted their vote.
    pub(crate) votes_granted: HashSet<NodeId>,
}

impl CandidateState {
    /// Returns true if the number of granted votes meets or exceeds `quorum`.
    pub(crate) fn has_majority(&self, quorum: usize) -> bool {
        self.votes_granted.len() >= quorum
    }
}

/// Raft volatile state
#[derive(Debug, Clone, Default)]
pub struct VolatileState {
    /// index of highest log entry known to be committed
    pub(crate) commit_index: LogIndex,
    /// index of highest log entry applied to state machine
    pub(crate) last_applied: LogIndex,
}

/// Raft leader volatile state (next_index and match_index per *follower*, self excluded)
#[derive(Debug, Clone)]
pub struct LeaderState {
    /// next_index[i] is the next log index to send to peers[i]
    pub next_index: Vec<LogIndex>,
    /// match_index[i] is the highest log index known to be replicated on peers[i]
    pub match_index: Vec<LogIndex>,
}

/// Raft core struct
pub struct Raft {
    pub(crate) id: NodeId,
    pub(crate) peers: Vec<NodeId>,
    pub(crate) role: Role,
    pub(crate) persistent: PersistentState,
    pub(crate) volatile: VolatileState,
    pub(crate) leader_state: Option<LeaderState>,
    pub(crate) candidate_state: Option<CandidateState>,
}

impl Raft {
    /// Create a new Raft node
    pub fn new(id: NodeId, peers: Vec<NodeId>) -> Self {
        debug_assert!(
            !peers.contains(&id),
            "peers must not contain self (id={id})"
        );
        Self {
            id,
            peers,
            role: Role::Follower,
            persistent: PersistentState::default(),
            volatile: VolatileState::default(),
            leader_state: None,
            candidate_state: None,
        }
    }

    /// Quorum size: how many votes (including self) are needed to win.
    ///
    /// With N peers (excluding self), cluster size = N+1, majority = ⌊(N+1)/2⌋ + 1.
    pub(crate) fn quorum(&self) -> usize {
        self.peers.len().div_ceil(2) + 1
    }

    /// Index of the last log entry, or 0 if the log is empty.
    pub(crate) fn last_log_index(&self) -> LogIndex {
        self.persistent.log.last().map_or(0, |e| e.index)
    }

    /// Term of the last log entry, or 0 if the log is empty.
    pub(crate) fn last_log_term(&self) -> Term {
        self.persistent.log.last().map_or(0, |e| e.term)
    }
}
