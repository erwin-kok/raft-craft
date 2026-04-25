use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use raft_core::protocol::{message::Message, types::NodeId};
use raft_runtime::{Clock, Transport};
use rand::RngExt;
use tokio::sync::mpsc;
use tracing::trace;

use crate::clock::SimulatedClock;

// ── IncomingSender ────────────────────────────────────────────────────────────

/// The sender half of a node's incoming-message channel.
/// `Network` holds one per registered (live) node.
pub type IncomingSender = mpsc::Sender<(NodeId, Message)>;

// ── NetworkConfig ─────────────────────────────────────────────────────────────

/// Per-link or global network fault configuration.
#[derive(Debug, Clone, Default)]
pub struct NetworkConfig {
    /// Probability [0.0, 1.0] that any individual message is silently dropped.
    pub drop_rate: f64,
    /// Fixed one-way latency added to every message (simulated nanoseconds).
    /// `0` means immediate delivery.
    pub latency_ms: u64,
}

// ── Network ───────────────────────────────────────────────────────────────────

/// Simulated network that connects all nodes in the cluster.
///
/// `Network` implements `raft_runtime::Transport` so it can be passed directly
/// to `Node::new`.  Every call to `send` goes through the fault-injection
/// pipeline before (optionally) pushing the message into the target node's
/// incoming channel.
///
/// Fault-injection pipeline (applied in order):
/// 1. **Dead node check** — target node not registered → drop silently.
/// 2. **Partition check** — `(from, to)` link is partitioned → drop silently.
/// 3. **Drop rate** — random drop according to `NetworkConfig::drop_rate`.
/// 4. **Latency** — if `latency_ms > 0`, spawn a task that sleeps for that
///    many simulated nanoseconds before delivering the message.
/// 5. **Delivery** — push `(from, msg)` into the target's channel.
///
/// The `from` identity is tracked per `Network` instance (one per node).
/// Use `Network::for_node(id)` to get the transport handle for node `id`.
#[derive(Clone)]
pub struct Network {
    inner: Arc<NetworkInner>,
    from_id: NodeId,
}

struct NetworkInner {
    /// Live node channels.
    senders: Mutex<HashMap<NodeId, IncomingSender>>,
    /// Pairs (a, b) where messages from a to b are dropped.
    partitions: Mutex<HashSet<(NodeId, NodeId)>>,
    /// Global fault config (per-link overrides could be added later).
    config: Mutex<NetworkConfig>,
    /// Clock used for latency simulation.
    clock: SimulatedClock,
}

impl Network {
    pub fn new(clock: SimulatedClock) -> Arc<Self> {
        // `from_id` is filled in by `for_node`; start at 0 (unused).
        Arc::new(Self {
            inner: Arc::new(NetworkInner {
                senders: Mutex::new(HashMap::new()),
                partitions: Mutex::new(HashSet::new()),
                config: Mutex::new(NetworkConfig::default()),
                clock,
            }),
            from_id: 0,
        })
    }

    /// Return a `Network` handle that records `from_id` as the sender.
    /// This is what gets passed to each individual `Node::new`.
    pub fn for_node(self: &Arc<Self>, from_id: NodeId) -> Arc<Self> {
        Arc::new(Self {
            inner: self.inner.clone(),
            from_id,
        })
    }

    // ── registration ─────────────────────────────────────────────────────────

    /// Register the incoming channel for `node_id`.
    /// Call before the node's task starts.
    pub fn register(&self, node_id: NodeId, tx: IncomingSender) {
        self.inner.senders.lock().unwrap().insert(node_id, tx);
    }

    /// Deregister a node — its messages will be dropped from now on.
    /// Call immediately after aborting the node's task.
    pub fn deregister(&self, node_id: NodeId) {
        self.inner.senders.lock().unwrap().remove(&node_id);
    }

    // ── fault injection controls ──────────────────────────────────────────────

    /// Drop all messages between `a` and `b` in both directions.
    pub fn partition(&self, a: NodeId, b: NodeId) {
        let mut p = self.inner.partitions.lock().unwrap();
        p.insert((a, b));
        p.insert((b, a));
    }

    /// Restore the link between `a` and `b`.
    pub fn heal(&self, a: NodeId, b: NodeId) {
        let mut p = self.inner.partitions.lock().unwrap();
        p.remove(&(a, b));
        p.remove(&(b, a));
    }

    /// Set global drop rate (0.0 = no drops, 1.0 = drop everything).
    pub fn set_drop_rate(&self, rate: f64) {
        self.inner.config.lock().unwrap().drop_rate = rate.clamp(0.0, 1.0);
    }

    /// Read the current global drop rate.
    pub fn drop_rate(&self) -> f64 {
        self.inner.config.lock().unwrap().drop_rate
    }

    /// Set global one-way latency in simulated milliseconds.
    /// `0` means immediate delivery (default).
    pub fn set_latency_ms(&self, ms: u64) {
        self.inner.config.lock().unwrap().latency_ms = ms;
    }

    /// Read the current global latency in simulated milliseconds.
    pub fn latency_ms(&self) -> u64 {
        self.inner.config.lock().unwrap().latency_ms
    }

    // ── delivery ──────────────────────────────────────────────────────────────

    fn deliver(&self, to: NodeId, msg: Message) {
        let from = self.from_id;

        // 1. Dead node check.
        let tx = {
            let senders = self.inner.senders.lock().unwrap();
            match senders.get(&to).cloned() {
                Some(tx) => tx,
                None => {
                    trace!("network: drop {from}→{to} (node dead)");
                    return;
                }
            }
        };

        // 2. Partition check.
        if self.inner.partitions.lock().unwrap().contains(&(from, to)) {
            trace!("network: drop {from}→{to} (partitioned)");
            return;
        }

        // 3. Drop rate.
        let (drop_rate, latency_ms) = {
            let cfg = self.inner.config.lock().unwrap();
            (cfg.drop_rate, cfg.latency_ms)
        };
        if drop_rate > 0.0 && rand::rng().random::<f64>() < drop_rate {
            trace!("network: drop {from}→{to} (random drop)");
            return;
        }

        // 4. Latency.
        if latency_ms > 0 {
            let clock = self.inner.clock.clone();
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                clock.sleep(Duration::from_millis(latency_ms)).await;
                let _ = tx_clone.try_send((from, msg));
            });
        } else {
            // 5. Immediate delivery.
            if let Err(e) = tx.try_send((from, msg)) {
                trace!("network: drop {from}→{to} (channel full: {e})");
            }
        }
    }
}

impl Transport for Network {
    fn send(&self, to: NodeId, msg: Message) {
        self.deliver(to, msg);
    }
}

// ── NodeNetwork ───────────────────────────────────────────────────────────────

/// Newtype wrapper so we can implement the foreign `Transport` trait on
/// `Arc<Network>` without violating the orphan rule.
///
/// `cluster.rs` creates one `NodeNetwork` per node and passes it to `Node::new`.
#[derive(Clone)]
pub struct NodeNetwork(pub Arc<Network>);

impl Transport for NodeNetwork {
    fn send(&self, to: NodeId, msg: Message) {
        self.0.deliver(to, msg);
    }
}
