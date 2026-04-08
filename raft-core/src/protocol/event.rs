use crate::protocol::{command::Command, message::Message};

#[derive(Debug, Clone)]
pub enum Event {
    Message(Message),
    ElectionTimeout,
    HeartbeatTimeout,
    ClientRequest(Command),
}

