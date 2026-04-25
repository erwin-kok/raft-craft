mod clock;
mod cluster;
mod network;
mod state_machine;
mod ui;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use clock::{SimulatedClock, spawn_auto_advance};
use cluster::Cluster;
use raft_runtime::NodeConfig;
use std::time::Duration;

/// Real-time nanoseconds between clock ticks (10000 ns = 100 ticks/sec).
const TICK_INTERVAL_NS: u64 = 10000;

fn main() -> Result<()> {
    // Tracing — set RUST_LOG=info or RUST_LOG=debug to see node events.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    // Use a multi-threaded runtime so node tasks, the clock task, and the UI
    // all make progress concurrently.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<()> {
    // Simulator-tuned config: faster timeouts than production defaults so
    // elections are visible in the UI within a second or two.
    let config = NodeConfig {
        election_timeout_min: Duration::from_millis(150),
        election_timeout_max: Duration::from_millis(300),
        heartbeat_interval: Duration::from_millis(50),
        incoming_channel_capacity: 256,
        client_channel_capacity: 64,
    };

    let clock = SimulatedClock::new();

    // Start the background task that advances the clock automatically.
    let _advance = spawn_auto_advance(clock.clone(), TICK_INTERVAL_NS);

    // 5-node cluster.
    let node_ids: Vec<u64> = (1..=5).collect();
    let mut cluster = Cluster::new(node_ids, clock, config);

    // Run the terminal UI.  This blocks until the user presses 'q'.
    ui::run(&mut cluster)?;

    Ok(())
}
