use crate::{
    Raft,
    protocol::{
        action::Action,
        message::{RequestVote, RequestVoteResponse},
    },
};

impl Raft {
    pub(crate) fn handle_request_vote(&mut self, _msg: RequestVote) -> Vec<Action> {
        vec![]
    }

    pub(crate) fn handle_vote_response(&mut self, _msg: RequestVoteResponse) -> Vec<Action> {
        vec![]
    }
}
