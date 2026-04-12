use std::{
    collections::VecDeque,
    sync::{Arc, RwLock},
};

use raft_core::{Role, protocol::types::NodeId};

const MESSAGE_LOG_CAP: usize = 200;

// ── MetricsSnapshot ───────────────────────────────────────────────────────────

/// A point-in-time snapshot of a node's observable state.
///
/// Readers clone this cheaply — all fields are small.
/// `message_log` is ordered oldest-first, newest-last.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub node_id: NodeId,
    pub role: Role,
    pub term: u64,
    pub commit_index: u64,
    pub last_applied: u64,
    pub log_length: u64,
    pub leader_id: Option<NodeId>,
    /// Recent outgoing messages, oldest first, capped at `MESSAGE_LOG_CAP`.
    pub message_log: Vec<MessageEvent>,
}

// ── MessageEvent ──────────────────────────────────────────────────────────────

/// A single entry in the message log.
#[derive(Debug, Clone)]
pub struct MessageEvent {
    pub tick: u64,
    pub from: NodeId,
    pub to: NodeId,
    /// Human-readable label e.g. `"AppendEntries(t=3, n=1)"`.
    pub label: String,
}

// ── Inner state ───────────────────────────────────────────────────────────────

struct Inner {
    node_id: NodeId,
    role: Role,
    term: u64,
    commit_index: u64,
    last_applied: u64,
    log_length: u64,
    leader_id: Option<NodeId>,
    /// Ring buffer — O(1) push_back and pop_front.
    message_log: VecDeque<MessageEvent>,
}

impl Inner {
    fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            role: Role::Follower,
            term: 0,
            commit_index: 0,
            last_applied: 0,
            log_length: 0,
            leader_id: None,
            message_log: VecDeque::new(),
        }
    }

    fn to_snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            node_id: self.node_id,
            role: self.role,
            term: self.term,
            commit_index: self.commit_index,
            last_applied: self.last_applied,
            log_length: self.log_length,
            leader_id: self.leader_id,
            message_log: self.message_log.iter().cloned().collect(),
        }
    }
}

// ── Metrics ───────────────────────────────────────────────────────────────────

/// Shared, readable metrics for a single node.
///
/// The node holds the only writer path (via `pub(crate)` methods).
/// All external readers call `snapshot()` which takes a brief read lock
/// and clones the small struct.
pub struct Metrics {
    inner: RwLock<Inner>,
}

impl Metrics {
    pub fn new(node_id: NodeId) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(Inner::new(node_id)),
        })
    }

    /// Read a full snapshot.  Cheap — a brief read lock and a clone.
    pub fn snapshot(&self) -> MetricsSnapshot {
        self.inner.read().unwrap().to_snapshot()
    }

    /// Update all scalar fields atomically after a `step()` call.
    /// Only called from the node's single-threaded event loop.
    pub(crate) fn update(
        &self,
        role: Role,
        term: u64,
        commit_index: u64,
        last_applied: u64,
        log_length: u64,
        leader_id: Option<NodeId>,
    ) {
        let mut inner = self.inner.write().unwrap();
        inner.role = role;
        inner.term = term;
        inner.commit_index = commit_index;
        inner.last_applied = last_applied;
        inner.log_length = log_length;
        inner.leader_id = leader_id;
    }

    /// Append a message event. Evicts the oldest entry when the cap is reached.
    pub(crate) fn record_message(&self, event: MessageEvent) {
        let mut inner = self.inner.write().unwrap();
        if inner.message_log.len() >= MESSAGE_LOG_CAP {
            inner.message_log.pop_front();
        }
        inner.message_log.push_back(event);
    }
}
