use std::{future::Future, pin::Pin, time::Duration};
use tokio::time;

pub type SleepFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Abstraction over the passage of time.
pub trait Clock: Send + Sync + 'static {
    fn sleep(&self, duration: Duration) -> SleepFuture;
}

// ── RealClock ─────────────────────────────────────────────────────────────────

/// Wall-clock implementation for production use. Delegates to `tokio::time`
#[derive(Clone, Default)]
pub struct RealClock;

impl Clock for RealClock {
    fn sleep(&self, duration: Duration) -> SleepFuture {
        Box::pin(time::sleep(duration))
    }
}
