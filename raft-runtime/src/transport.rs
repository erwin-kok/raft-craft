use raft_core::protocol::{message::Message, types::NodeId};

/// Abstraction over message delivery between nodes.
///
/// The runtime calls `send` when the Raft core emits an `Action::Send`.
/// The runtime receives messages via a plain `mpsc::Receiver<(NodeId, Message)>`
/// that is created externally (by the transport implementation) and handed to
/// `Node::new` as `incoming`.  This keeps the node free of any knowledge about
/// how messages are delivered, buffered, delayed, or dropped.
///
/// # Implementing Transport
///
/// The implementor must:
/// 1. Create a `mpsc::channel` for each node it manages.
/// 2. Pass the `Receiver` end to `Node::new` as the `incoming` parameter.
/// 3. Push `(from, message)` tuples into the `Sender` end whenever a message
///    arrives for that node (from network I/O, from another in-process node,
///    after a simulated delay, etc.).
/// 4. Implement `send` to deliver outgoing messages however is appropriate.
pub trait Transport: Send + Sync + 'static {
    /// Deliver `msg` from this node to node `to`.
    /// Fire-and-forget: the implementor is responsible for error handling.
    fn send(&self, to: NodeId, msg: Message);
}
