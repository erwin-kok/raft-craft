use crate::{
    Raft,
    protocol::{
        action::Action,
        message::{AppendEntries, AppendEntriesResponse},
    },
};

impl Raft {
    pub(crate) fn handle_append_entries(&mut self, _msg: AppendEntries) -> Vec<Action> {
        vec![]
    }

    pub(crate) fn handle_append_entries_response(
        &mut self,
        _msg: AppendEntriesResponse,
    ) -> Vec<Action> {
        vec![]
    }
}
