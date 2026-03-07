use crate::{Raft, protocol::action::Action};

impl Raft {
    pub(crate) fn handle_heartbeat_timeout(&mut self) -> Vec<Action> {
        vec![]
    }
}
