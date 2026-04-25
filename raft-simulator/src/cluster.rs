use std::{collections::HashMap, sync::Arc};

use raft_core::protocol::{command::Command, types::NodeId};
use raft_runtime::{
    Metrics, NoPersistence, NodeConfig,
    node::{ClientRequest, ClientResponse},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::info;

use crate::{
    clock::SimulatedClock,
    network::{Network, NodeNetwork},
    state_machine::KeyValueStore,
};

// ── NodeHandle ────────────────────────────────────────────────────────────────

/// Everything the cluster manager and UI need to interact with one node.
pub struct NodeHandle {
    pub metrics: Arc<Metrics>,
    /// Send client commands into the node's event loop.
    pub client_tx: mpsc::Sender<ClientRequest>,
    /// The running tokio task.  `None` when the node is dead.
    task: Option<JoinHandle<()>>,
    pub alive: bool,
}

// ── Cluster ───────────────────────────────────────────────────────────────────

/// Manages the full simulated cluster: nodes, network, and clock.
pub struct Cluster {
    pub nodes: HashMap<NodeId, NodeHandle>,
    pub network: Arc<Network>,
    pub clock: SimulatedClock,
    /// Sorted list of all node ids (including dead ones) for stable UI ordering.
    node_ids: Vec<NodeId>,
    /// Config template — all nodes use this; override before calling `new`.
    config: NodeConfig,
}

impl Cluster {
    /// Build a cluster and start all node tasks.
    pub fn new(node_ids: Vec<NodeId>, clock: SimulatedClock, config: NodeConfig) -> Self {
        let network = Network::new(clock.clone());
        let mut nodes = HashMap::new();

        for &id in &node_ids {
            let peers: Vec<NodeId> = node_ids.iter().copied().filter(|&p| p != id).collect();
            let handle = Self::start_node(id, peers, &network, &clock, &config);
            nodes.insert(id, handle);
        }

        Self {
            nodes,
            network,
            clock,
            node_ids,
            config,
        }
    }

    // ── node lifecycle ────────────────────────────────────────────────────────

    /// Hard-kill a node: abort its task and deregister it from the network.
    /// In-flight messages to/from this node are silently dropped.
    pub fn kill_node(&mut self, id: NodeId) {
        if let Some(h) = self.nodes.get_mut(&id) {
            if !h.alive {
                return;
            }
            if let Some(task) = h.task.take() {
                task.abort();
            }
            h.alive = false;
            self.network.deregister(id);
            info!("node {id} killed");
        }
    }

    /// Restart a previously killed node from a clean state (no persistence).
    pub fn restart_node(&mut self, id: NodeId) {
        if self.nodes.get(&id).map(|h| h.alive).unwrap_or(false) {
            return; // already running
        }
        let peers: Vec<NodeId> = self.node_ids.iter().copied().filter(|&p| p != id).collect();
        let handle = Self::start_node(id, peers, &self.network, &self.clock, &self.config);
        self.nodes.insert(id, handle);
        info!("node {id} restarted");
    }

    // ── network fault injection ───────────────────────────────────────────────

    /// Isolate `node` from every other node in the cluster.
    /// All messages to and from `node` will be dropped.
    pub fn isolate(&self, node: NodeId) {
        for &other in &self.node_ids {
            if other != node {
                self.network.partition(node, other);
            }
        }
        info!("node {node} isolated");
    }

    /// Restore all links to/from `node`.
    pub fn unisolate(&self, node: NodeId) {
        for &other in &self.node_ids {
            if other != node {
                self.network.heal(node, other);
            }
        }
        info!("node {node} unisolated");
    }

    /// Set global packet drop probability (0.0–1.0).
    pub fn set_drop_rate(&self, rate: f64) {
        self.network.set_drop_rate(rate);
    }

    /// Current global packet drop rate (0.0–1.0).
    pub fn drop_rate(&self) -> f64 {
        self.network.drop_rate()
    }

    /// Set global one-way latency in simulated milliseconds.
    pub fn set_latency_ms(&self, ms: u64) {
        self.network.set_latency_ms(ms);
    }

    /// Current global one-way latency in simulated milliseconds.
    pub fn latency_ms(&self) -> u64 {
        self.network.latency_ms()
    }

    /// Current clock speed in simulated microseconds per tick.
    pub fn speed_μs(&self) -> u64 {
        self.clock.speed_μs()
    }

    /// Set clock speed (clamped to 1–10000 sim-μs per tick).
    pub fn set_speed_μs(&self, ns: u64) {
        self.clock.set_speed_μs(ns);
    }

    // ── client commands ───────────────────────────────────────────────────────

    /// Submit a command to node `id`.
    ///
    /// Returns a `oneshot::Receiver` that resolves to `ClientResponse::Applied`
    /// once the entry is committed, or `ClientResponse::NotLeader { leader_id }`
    /// if the node is not the leader.
    ///
    /// Returns `None` if the node is dead or the channel is full.
    pub fn submit(&self, id: NodeId, cmd: Command) -> Option<oneshot::Receiver<ClientResponse>> {
        let handle = self.nodes.get(&id)?;
        if !handle.alive {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        handle.client_tx.try_send((cmd, tx)).ok()?;
        Some(rx)
    }

    // ── metrics ───────────────────────────────────────────────────────────────

    /// Sorted node id list (stable across kill/restart).
    pub fn node_ids(&self) -> &[NodeId] {
        &self.node_ids
    }

    // ── private ───────────────────────────────────────────────────────────────

    fn start_node(
        id: NodeId,
        peers: Vec<NodeId>,
        network: &Arc<Network>,
        clock: &SimulatedClock,
        config: &NodeConfig,
    ) -> NodeHandle {
        let cap = config.incoming_channel_capacity;
        let cli_cap = config.client_channel_capacity;

        let (incoming_tx, incoming_rx) = mpsc::channel(cap);
        let (client_tx, client_rx) = mpsc::channel(cli_cap);

        // Register this node's incoming channel with the network.
        network.register(id, incoming_tx);

        // Each node gets its own Network handle that records `id` as the sender.
        // Wrapped in NodeNetwork so it satisfies the Transport trait bound.
        let node_transport = NodeNetwork(network.for_node(id));

        let metrics = Metrics::new(id);
        let clock_tick = clock.now_μs_arc();
        let arc_clock: Arc<dyn raft_runtime::Clock> = Arc::new(clock.clone());

        let node = raft_runtime::Node::new(
            id,
            peers,
            None, // no persisted state on (re)start
            node_transport,
            incoming_rx,
            client_rx,
            NoPersistence,
            KeyValueStore::new(),
            arc_clock,
            clock_tick,
            config.clone(),
            metrics.clone(),
        );

        let task = tokio::spawn(node.run());

        NodeHandle {
            metrics,
            client_tx,
            task: Some(task),
            alive: true,
        }
    }
}
