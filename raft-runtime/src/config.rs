use std::time::Duration;

#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Minimum randomised election timeout.
    pub election_timeout_min: Duration,
    /// Maximum randomised election timeout.
    pub election_timeout_max: Duration,
    /// Fixed interval between heartbeat broadcasts (leader only).
    pub heartbeat_interval: Duration,
    /// Capacity of the incoming-message channel.
    pub incoming_channel_capacity: usize,
    /// Capacity of the client-command channel.
    pub client_channel_capacity: usize,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            election_timeout_min: Duration::from_millis(150),
            election_timeout_max: Duration::from_millis(300),
            heartbeat_interval: Duration::from_millis(50),
            incoming_channel_capacity: 256,
            client_channel_capacity: 64,
        }
    }
}
