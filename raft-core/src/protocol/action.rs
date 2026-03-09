use crate::protocol::{command::Command, message::Message, types::NodeId};

pub enum Action {
    /// Send this message to another node
    Send(NodeId, Message),

    /// Reset the election timer
    ResetElectionTimer,

    /// Reset heartbeat timer
    ResetHeartbeatTimer,

    /// Persists state
    PersistState,

    /// Apply Command to the state machine
    ApplyCommand(Command),
}
