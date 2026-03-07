use crate::command::Command;
use crate::message::Message;

pub enum Event {
    Message(Message),
    ElectionTimeout,
    HeartbeatTimeout,
    ClientRequest(Command),
}
