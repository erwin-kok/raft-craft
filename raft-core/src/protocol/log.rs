use crate::protocol::types::{LogIndex, Term};

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub term: Term,
    pub index: LogIndex,
    pub _command: String,
}
