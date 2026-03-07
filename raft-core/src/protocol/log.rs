use crate::protocol::types::Term;

#[derive(Debug, Clone)]
pub struct LogEntry {
    _term: Term,
    _command: String,
}
