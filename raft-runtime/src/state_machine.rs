use raft_core::protocol::command::Command;

/// Abstraction over the replicated state machine.
///
/// `apply` is called by the node's event loop each time a log entry is
/// committed and ready to be executed.  Calls are strictly sequential and
/// in log-index order, so no synchronisation is needed inside the
/// implementation.
pub trait StateMachine: Send + Sync + 'static {
    fn apply(&mut self, command: Command);
}
