use crate::protocol::{
    command::Command,
    types::{LogIndex, Term},
};

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub term: Term,
    pub index: LogIndex,
    pub command: Command,
}
