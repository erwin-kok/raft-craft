/// Integration tests for `raft-runtime`.
///
/// These tests wire up one or more `Node` instances using:
/// - `NoPersistence`   — no disk I/O
/// - `RealClock`       — real tokio time (short timeouts)
/// - A trivial `RecordingStateMachine` — records applied commands
/// - A manual in-process transport — channels wired by hand
///
/// The tests verify that the runtime correctly routes actions, resolves
/// client responses, and that a single-node cluster applies commands.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};

use crate::{
    ClientRequest, Metrics, NodeConfig,
    clock::RealClock,
    node::{ClientResponse, Node},
    persistence::NoPersistence,
    state_machine::StateMachine,
    transport::Transport,
};
use raft_core::{
    Role,
    protocol::{command::Command, message::Message, types::NodeId},
};
use tokio::{
    sync::{mpsc, oneshot},
    time,
};

// ── RecordingStateMachine ─────────────────────────────────────────────────

#[derive(Clone, Default)]
struct RecordingStateMachine {
    applied: Arc<Mutex<Vec<Command>>>,
}

impl RecordingStateMachine {
    fn new() -> Self {
        Self::default()
    }
    fn applied(&self) -> Vec<Command> {
        self.applied.lock().unwrap().clone()
    }
}

impl StateMachine for RecordingStateMachine {
    fn apply(&mut self, command: Command) {
        self.applied.lock().unwrap().push(command);
    }
}

// ── ManualTransport ───────────────────────────────────────────────────────
//
// Each node registers its incoming sender here.  `send()` pushes directly
// into the target's channel, mimicking InMemoryTransport without any
// fault-injection.

type SenderType = (NodeId, Message);

#[derive(Clone, Default)]
struct ManualTransport {
    senders: Arc<Mutex<HashMap<NodeId, mpsc::Sender<SenderType>>>>,
    self_id: NodeId,
}

impl ManualTransport {
    fn new(self_id: NodeId) -> Self {
        Self {
            senders: Default::default(),
            self_id,
        }
    }
}

impl Transport for ManualTransport {
    fn send(&self, to: NodeId, msg: Message) {
        let from = self.self_id;
        if let Some(tx) = self.senders.lock().unwrap().get(&to).cloned() {
            let _ = tx.try_send((from, msg));
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────

/// Fast timeouts so tests complete quickly.
fn fast_config() -> NodeConfig {
    NodeConfig {
        election_timeout_min: Duration::from_millis(20),
        election_timeout_max: Duration::from_millis(40),
        heartbeat_interval: Duration::from_millis(10),
        incoming_channel_capacity: 64,
        client_channel_capacity: 16,
    }
}

type TestNode = Node<RecordingStateMachine, NoPersistence, ManualTransport>;

/// Build a node and return (node, client_tx, incoming_tx, state_machine).
fn build_node(
    id: NodeId,
    peers: Vec<NodeId>,
    transport: ManualTransport,
) -> (
    TestNode,
    mpsc::Sender<ClientRequest>,
    mpsc::Sender<SenderType>,
    RecordingStateMachine,
    Arc<Metrics>,
) {
    let config = fast_config();
    let cap = config.incoming_channel_capacity;
    let cli_cap = config.client_channel_capacity;

    let (incoming_tx, incoming_rx) = mpsc::channel::<SenderType>(cap);
    let (client_tx, client_rx) = mpsc::channel(cli_cap);
    let sm = RecordingStateMachine::new();
    let metrics = Metrics::new(id);
    let clock_tick = Arc::new(AtomicU64::new(0));

    let node = Node::new(
        id,
        peers,
        None,
        transport,
        incoming_rx,
        client_rx,
        NoPersistence,
        sm.clone(),
        Arc::new(RealClock),
        clock_tick,
        config,
        metrics.clone(),
    );

    (node, client_tx, incoming_tx, sm, metrics)
}

// ── tests ─────────────────────────────────────────────────────────────────

/// A single-node cluster should elect itself and apply a client command.
#[tokio::test]
async fn single_node_applies_command() {
    let transport = ManualTransport::new(1);
    let (node, client_tx, _incoming_tx, sm, _metrics) = build_node(1, vec![], transport);

    tokio::spawn(node.run());

    // Give the node time to elect itself (single-node → immediate).
    time::sleep(Duration::from_millis(60)).await;

    let cmd = Command::set("hello", "world");
    let (reply_tx, reply_rx) = oneshot::channel();
    client_tx.send((cmd.clone(), reply_tx)).await.unwrap();

    let response = time::timeout(Duration::from_millis(200), reply_rx)
        .await
        .expect("timed out waiting for client response")
        .expect("channel closed");

    assert!(
        matches!(response, ClientResponse::Applied),
        "expected Applied, got {response:?}"
    );
    assert_eq!(sm.applied(), vec![cmd]);
}

/// A non-leader node should immediately return `NotLeader`.
#[tokio::test]
async fn non_leader_returns_not_leader() {
    // Node 1 has peers [2] so it won't win an election alone — it stays follower.
    let transport = ManualTransport::new(1);
    let (node, client_tx, _incoming_tx, _sm, _metrics) = build_node(1, vec![2], transport);

    tokio::spawn(node.run());

    // Don't wait for an election — just send immediately while it's a follower.
    let (reply_tx, reply_rx) = oneshot::channel();
    client_tx
        .send((Command::set("k", "v"), reply_tx))
        .await
        .unwrap();

    let response = time::timeout(Duration::from_millis(100), reply_rx)
        .await
        .expect("timed out")
        .expect("channel closed");

    assert!(
        matches!(response, ClientResponse::NotLeader { .. }),
        "expected NotLeader, got {response:?}"
    );
}

/// Metrics should reflect the node's state after election.
#[tokio::test]
async fn metrics_updated_after_election() {
    let transport = ManualTransport::new(1);
    let (node, _client_tx, _incoming_tx, _sm, metrics) = build_node(1, vec![], transport);

    tokio::spawn(node.run());
    time::sleep(Duration::from_millis(80)).await;

    let snap = metrics.snapshot();
    assert_eq!(snap.node_id, 1);
    assert_eq!(snap.role, raft_core::Role::Leader);
    assert!(snap.term >= 1);
}

/// Three-node cluster: two nodes exchange messages and a leader is elected.
#[tokio::test]
async fn three_node_cluster_elects_leader() {
    // Shared sender map so all ManualTransports can route to each other.
    let shared = Arc::new(Mutex::new(HashMap::new()));

    // Build three transports that all share the same sender map.
    let make_transport = |id: NodeId| ManualTransport {
            senders: shared.clone(),
            self_id: id,
    };

    let config = fast_config();
    let cap = config.incoming_channel_capacity;

    let mut handles = vec![];
    let mut metrics_v = vec![];

    for id in 1u64..=3 {
        let peers: Vec<NodeId> = (1u64..=3).filter(|&p| p != id).collect();
        let transport = make_transport(id);

        let (incoming_tx, incoming_rx) = mpsc::channel::<SenderType>(cap);
        let (client_tx, client_rx) = mpsc::channel(16);

        // Register this node's incoming sender in the shared map.
        shared.lock().unwrap().insert(id, incoming_tx);

        let sm = RecordingStateMachine::new();
        let metrics = Metrics::new(id);
        let clock_tick = Arc::new(AtomicU64::new(0));

        let node: TestNode = Node::new(
            id,
            peers,
            None,
            transport,
            incoming_rx,
            client_rx,
            NoPersistence,
            sm,
            Arc::new(RealClock),
            clock_tick,
            config.clone(),
            metrics.clone(),
        );

        metrics_v.push(metrics);
        handles.push(tokio::spawn(node.run()));
        drop(client_tx); // not needed for this test
    }

    // Wait for an election to complete.
    time::sleep(Duration::from_millis(300)).await;

    let leader_count = metrics_v
        .iter()
        .filter(|m| m.snapshot().role == Role::Leader)
        .count();

    assert_eq!(leader_count, 1, "expected exactly one leader");

    // Abort all tasks.
    for h in handles {
        h.abort();
    }
}
