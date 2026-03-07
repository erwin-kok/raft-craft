use crate::protocol::{command::Command, message::Message};

pub enum Event {
    Message(Message),
    ElectionTimeout,
    HeartbeatTimeout,
    ClientRequest(Command),
}
