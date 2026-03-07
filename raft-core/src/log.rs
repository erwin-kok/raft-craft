use crate::types::Term;

#[derive(Debug, Clone)]
pub struct LogEntry {
    term: Term,
    command: String,
}
