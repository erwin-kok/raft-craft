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

    /// Notify client of actual leader
    NotLeader(Option<NodeId>),

    /// Apply Command to the state machine
    ApplyCommand(Command),
}
