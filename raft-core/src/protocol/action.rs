use crate::protocol::{command::Command, message::Message, types::NodeId};

#[derive(Debug, Clone)]
pub enum Action {
    /// Send a message to another node.
    Send(NodeId, Message),
    /// Reset (restart) the election timer.
    ResetElectionTimer,
    /// Reset (restart) the heartbeat timer.
    ResetHeartbeatTimer,
    /// Persist current term, voted_for, and log to stable storage.
    PersistState,
    /// Notify the client that this node is not the leader.
    /// Carries the best-known current leader id.
    NotLeader(Option<NodeId>),
    /// Apply this command to the state machine.
    ApplyCommand(Command),
}

