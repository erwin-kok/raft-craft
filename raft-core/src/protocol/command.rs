use serde::{Deserialize, Serialize};

/// A command submitted by a client and replicated through the Raft log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub op: Op,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    Set { key: String, value: String },
    Delete { key: String },
}

impl Command {
    pub fn set(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            op: Op::Set {
                key: key.into(),
                value: value.into(),
            },
        }
    }

    pub fn delete(key: impl Into<String>) -> Self {
        Self {
            op: Op::Delete { key: key.into() },
        }
    }
}

impl Default for Command {
    fn default() -> Self {
        Self::set("key", "value")
    }
}
