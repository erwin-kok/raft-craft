use crate::{
    Raft,
    protocol::{action::Action, command::Command},
};

impl Raft {
    pub(crate) fn handle_client_request(&mut self, _command: Command) -> Vec<Action> {
        vec![]
    }
}
