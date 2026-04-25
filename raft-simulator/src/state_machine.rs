use std::collections::HashMap;

use raft_core::protocol::command::{Command, Op};
use raft_runtime::StateMachine;

/// A simple in-memory key-value store.
///
/// This is the application state machine used by the simulator.  Every
/// committed `Command` is applied here in log-index order by the `Node`
/// event loop.
#[derive(Debug, Default, Clone)]
pub struct KeyValueStore {
    data: HashMap<String, String>,
}

impl KeyValueStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StateMachine for KeyValueStore {
    fn apply(&mut self, command: Command) {
        match command.op {
            Op::Set { key, value } => {
                self.data.insert(key, value);
            }
            Op::Delete { key } => {
                self.data.remove(&key);
            }
        }
    }
}
