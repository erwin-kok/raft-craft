use std::{
    collections::HashMap,
    mem,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};

use raft_core::{
    PersistentState, Raft,
    protocol::{
        action::Action,
        command::Command,
        event::Event,
        message::Message,
        types::{LogIndex, NodeId},
    },
};

use crate::{
    Clock,
    config::NodeConfig,
    metrics::{MessageEvent, Metrics},
    persistence::Persistence,
    state_machine::StateMachine,
    timer::{ElectionTimer, HeartbeatTimer, TimerEvent},
    transport::Transport,
};

// ── Public channel types ──────────────────────────────────────────────────────

/// A message arriving at this node: `(sender_id, message)`.
pub type Incoming = mpsc::Receiver<(NodeId, Message)>;

/// A client command paired with a channel to deliver the result.
pub type ClientRequest = (Command, oneshot::Sender<ClientResponse>);

/// The result of a client command, returned once the entry is applied (or
/// redirected when this node is not the leader).
#[derive(Debug)]
pub enum ClientResponse {
    /// Command was committed and applied to the state machine.
    Applied,
    /// This node is not the leader.  `leader_id` is our best guess at the
    /// current leader — `None` if no leader has been observed yet.
    NotLeader { leader_id: Option<NodeId> },
}

// ── Pending client tracking ───────────────────────────────────────────────────

struct PendingClient {
    tx: oneshot::Sender<ClientResponse>,
}

// ── Node ──────────────────────────────────────────────────────────────────────

/// A running Raft node.
///
/// # Channels
///
/// Three channels feed the event loop:
///
/// - **`incoming`** — `(NodeId, Message)` tuples pushed by the transport
///   implementation whenever a message arrives for this node.
/// - **`client_rx`** — `(Command, oneshot::Sender<ClientResponse>)` tuples
///   submitted by the application layer (e.g. an HTTP handler, a test harness,
///   or the simulator UI).
/// - **`timer_rx`** — internal; fed by `ElectionTimer` and `HeartbeatTimer`.
///
/// # Clock tick counter
///
/// `clock_tick` is an `Arc<AtomicU64>` that the simulator advances along with
/// its `SimulatedClock`.  The node reads it only to timestamp `MessageEvent`
/// entries in the metrics log.  Pass `Arc::new(AtomicU64::new(0))` in
/// production; the value is never critical to correctness.
pub struct Node<S: StateMachine, P: Persistence, T: Transport> {
    // ── core ─────────────────────────────────────────────────────────────────
    raft: Raft,
    transport: T,
    persistence: P,
    state_machine: S,
    metrics: Arc<Metrics>,

    // ── channels ─────────────────────────────────────────────────────────────
    incoming: Incoming,
    client_rx: mpsc::Receiver<ClientRequest>,
    timer_rx: mpsc::Receiver<TimerEvent>,

    // ── timers ────────────────────────────────────────────────────────────────
    election_timer: ElectionTimer,
    heartbeat_timer: HeartbeatTimer,

    // ── pending client responses ──────────────────────────────────────────────
    /// Maps log index → pending client sender, resolved when that index is applied.
    pending: HashMap<LogIndex, PendingClient>,

    // ── clock tick for message event timestamps ───────────────────────────────
    clock_tick: Arc<AtomicU64>,
}

impl<S: StateMachine, P: Persistence, T: Transport> Node<S, P, T> {
    /// Create a new node.
    ///
    /// # Parameters
    ///
    /// - `id`            — this node's logical identity.
    /// - `peers`         — identities of all *other* nodes in the cluster.
    /// - `initial_state` — recovered `PersistentState` from `persistence.load()`, or `None` for a fresh node.
    /// - `transport`     — used to send outgoing messages.
    /// - `incoming`      — channel on which the transport delivers inbound messages.
    /// - `client_rx`     — channel on which the application submits commands.
    /// - `persistence`   — where to durably write state.
    /// - `state_machine` — the application's replicated state machine.
    /// - `clock`         — controls time; use `RealClock` in production or a `SimulatedClock` in tests.
    /// - `clock_tick`    — shared tick counter for message-event timestamps.
    /// - `config`        — tunable timeouts and channel capacities.
    /// - `metrics`       — shared metrics object; also readable externally.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: NodeId,
        peers: Vec<NodeId>,
        initial_state: Option<PersistentState>,
        transport: T,
        incoming: Incoming,
        client_rx: mpsc::Receiver<ClientRequest>,
        persistence: P,
        state_machine: S,
        clock: Arc<dyn Clock>,
        clock_tick: Arc<AtomicU64>,
        config: NodeConfig,
        metrics: Arc<Metrics>,
    ) -> Self {
        let raft = match initial_state {
            Some(state) => Raft::restore(id, peers, state),
            None => Raft::new(id, peers),
        };

        // The timers communicate with the event loop through their own
        // internal channel so they never need to know about the other channels.
        let (timer_tx, timer_rx) = mpsc::channel(16);

        let election_timer = ElectionTimer::new(
            clock.clone(),
            timer_tx.clone(),
            config.election_timeout_min,
            config.election_timeout_max,
        );
        let heartbeat_timer = HeartbeatTimer::new(clock, timer_tx, config.heartbeat_interval);

        Self {
            raft,
            transport,
            persistence,
            state_machine,
            metrics,
            incoming,
            client_rx,
            timer_rx,
            election_timer,
            heartbeat_timer,
            pending: HashMap::new(),
            clock_tick,
        }
    }

    /// Run the node's event loop.
    ///
    /// Returns when all input channels are closed — normally when the
    /// simulator drops or aborts the task to simulate a crash.
    pub async fn run(mut self) {
        self.election_timer.reset();
        self.update_metrics();
        info!(
            "node {} started (term={})",
            self.raft.id, self.raft.persistent.current_term
        );

        loop {
            tokio::select! {
                // Timer events (election timeout, heartbeat timeout)
                Some(timer_event) = self.timer_rx.recv() => {
                    let event = match timer_event {
                        TimerEvent::ElectionTimeout  => Event::ElectionTimeout,
                        TimerEvent::HeartbeatTimeout => Event::HeartbeatTimeout,
                    };
                    self.step(event).await;
                }

                // Inbound messages from other nodes via the transport
                Some((from, msg)) = self.incoming.recv() => {
                    info!("node {} recv from {}: {}", self.raft.id, from, msg_label(&msg));
                    self.step(Event::Message(msg)).await;
                }

                // Client commands submitted by the application layer
                Some((cmd, reply_tx)) = self.client_rx.recv() => {
                    self.handle_client(cmd, reply_tx).await;
                }

                else => {
                    info!("node {} all channels closed, stopping", self.raft.id);
                    break;
                }
            }
        }
    }

    // ── internal step ─────────────────────────────────────────────────────────

    async fn step(&mut self, event: Event) {
        let actions = self.raft.step(event);
        self.process_actions(actions).await;
        self.update_metrics();
    }

    // ── client handling ───────────────────────────────────────────────────────

    async fn handle_client(&mut self, cmd: Command, reply_tx: oneshot::Sender<ClientResponse>) {
        // Snapshot the log length before stepping so we can tell whether the
        // command was actually appended (leader) or rejected (not leader).
        let before = self.raft.last_log_index();
        let actions = self.raft.step(Event::ClientRequest(cmd));

        if self.raft.last_log_index() > before {
            // Leader accepted the command. Park the response channel at the
            // new log index; it is resolved when ApplyCommand fires for that index.
            self.pending
                .insert(self.raft.last_log_index(), PendingClient { tx: reply_tx });
        } else {
            // Not the leader. process_actions will emit Action::NotLeader which
            // drains pending — but we didn't insert, so just reply directly.
            let _ = reply_tx.send(ClientResponse::NotLeader {
                leader_id: self.raft.known_leader,
            });
        }

        self.process_actions(actions).await;
        self.update_metrics();
    }

    // ── action dispatch ───────────────────────────────────────────────────────

    async fn process_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Send(to, msg) => {
                    self.record_outgoing(&msg, to);
                    self.transport.send(to, msg);
                }

                Action::ResetElectionTimer => {
                    self.election_timer.reset();
                }

                Action::ResetHeartbeatTimer => {
                    // Cancel the election timer while we are leader.
                    self.election_timer.cancel();
                    self.heartbeat_timer.reset();
                }

                Action::PersistState => {
                    // Synchronous write — must complete before we send responses.
                    if let Err(e) = self.persistence.save(&self.raft.persistent) {
                        error!("node {} persist error: {e}", self.raft.id);
                    }
                }

                Action::NotLeader(leader_id) => {
                    // Drain any pending client requests accumulated while we
                    // thought we were leader (shouldn't normally happen, but
                    // handles the race between a step-down and an in-flight client).
                    let pending = mem::take(&mut self.pending);
                    for (_, client) in pending {
                        let _ = client.tx.send(ClientResponse::NotLeader { leader_id });
                    }
                }

                Action::ApplyCommand(cmd) => {
                    // last_applied has already been incremented by the Raft core.
                    let index = self.raft.volatile.last_applied;
                    self.state_machine.apply(cmd);

                    // Resolve the pending client for this index, if any.
                    if let Some(client) = self.pending.remove(&index) {
                        let _ = client.tx.send(ClientResponse::Applied);
                    }
                }
            }
        }
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn update_metrics(&self) {
        self.metrics.update(
            self.raft.role,
            self.raft.persistent.current_term,
            self.raft.volatile.commit_index,
            self.raft.volatile.last_applied,
            self.raft.persistent.log.len() as u64,
            self.raft.known_leader,
        );
    }

    fn record_outgoing(&self, msg: &Message, to: NodeId) {
        let tick = self.clock_tick.load(Ordering::Relaxed);
        self.metrics.record_message(MessageEvent {
            tick,
            from: self.raft.id,
            to,
            label: msg_label(msg),
        });
    }
}

// ── label helper ─────────────────────────────────────────────────────────────

fn msg_label(msg: &Message) -> String {
    match msg {
        Message::RequestVote(m) => format!("RequestVote(t={})", m.term),
        Message::RequestVoteResponse(m) => {
            format!("VoteResponse(t={}, granted={})", m.term, m.vote_granted)
        }
        Message::AppendEntries(m) => format!("AppendEntries(t={}, n={})", m.term, m.entries.len()),
        Message::AppendEntriesResponse(m) => {
            format!("AppendEntriesResponse(t={}, ok={})", m.term, m.success)
        }
    }
}
