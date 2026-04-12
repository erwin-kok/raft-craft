use rand::Rng;
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::clock::Clock;

// ── TimerEvent ────────────────────────────────────────────────────────────────

/// Internal event variants fired by timers into the node's event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimerEvent {
    ElectionTimeout,
    HeartbeatTimeout,
}

// ── ElectionTimer ─────────────────────────────────────────────────────────────

/// Fires `TimerEvent::ElectionTimeout` after a random delay in
/// `[min, max]`.
///
/// Every `reset()` call aborts the previous pending task and starts a new
/// one, so exactly one timer task is live at any time.
pub(crate) struct ElectionTimer {
    clock: Arc<dyn Clock>,
    event_tx: mpsc::Sender<TimerEvent>,
    min: Duration,
    max: Duration,
    handle: Option<JoinHandle<()>>,
}

impl ElectionTimer {
    pub fn new(
        clock: Arc<dyn Clock>,
        event_tx: mpsc::Sender<TimerEvent>,
        min: Duration,
        max: Duration,
    ) -> Self {
        Self {
            clock,
            event_tx,
            min,
            max,
            handle: None,
        }
    }

    pub fn reset(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
        let range_ms = self.min.as_millis() as u64..=self.max.as_millis() as u64;
        let ms = rand::thread_rng().gen_range(range_ms);
        let sleep = self.clock.sleep(Duration::from_millis(ms));
        let tx = self.event_tx.clone();
        self.handle = Some(tokio::spawn(async move {
            sleep.await;
            let _ = tx.send(TimerEvent::ElectionTimeout).await;
        }));
    }

    /// Stop the timer without rescheduling (e.g. node became leader).
    pub fn cancel(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

// ── HeartbeatTimer ────────────────────────────────────────────────────────────

/// Fires `TimerEvent::HeartbeatTimeout` after a fixed interval.
pub(crate) struct HeartbeatTimer {
    clock: Arc<dyn Clock>,
    event_tx: mpsc::Sender<TimerEvent>,
    interval: Duration,
    handle: Option<JoinHandle<()>>,
}

impl HeartbeatTimer {
    pub fn new(
        clock: Arc<dyn Clock>,
        event_tx: mpsc::Sender<TimerEvent>,
        interval: Duration,
    ) -> Self {
        Self {
            clock,
            event_tx,
            interval,
            handle: None,
        }
    }

    pub fn reset(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
        let sleep = self.clock.sleep(self.interval);
        let tx = self.event_tx.clone();
        self.handle = Some(tokio::spawn(async move {
            sleep.await;
            let _ = tx.send(TimerEvent::HeartbeatTimeout).await;
        }));
    }
}
