use crate::types::NodeId;
use crate::{log::LogEntry, message::Message};

pub enum Action {
    /// Send this message to another node
    Send(NodeId, Message),

    /// Reset the election timer
    ResetElectionTimer,

    /// Reset heartbeat timer
    ResetHeartbeatTimer,

    /// Persists state
    PersistState,

    /// Apply this log entry to the state machine
    ApplyLog(LogEntry),
}
