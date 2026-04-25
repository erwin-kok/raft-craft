use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

use raft_runtime::{Clock, SleepFuture};

// ── SimulatedClock ────────────────────────────────────────────────────────────

/// A deterministic clock whose time only advances when `tick()` is called.
///
/// `step_μs` controls how many simulated milliseconds each auto-advance tick
/// adds.  Increasing it speeds the simulation up; decreasing it slows it down.
/// It can be changed at any time via `set_speed_μs()`.
#[derive(Clone)]
pub struct SimulatedClock {
    inner: Arc<Inner>,
}

struct Inner {
    now_μs: Arc<AtomicU64>,
    paused: AtomicBool,
    /// Simulated μs added per auto-advance tick (adjusted by +/- keys).
    step_μs: AtomicU64,
    wakers: Mutex<BinaryHeap<Reverse<WakeEntry>>>,
}

// ── WakeEntry ─────────────────────────────────────────────────────────────────

struct WakeEntry {
    deadline_μs: u64,
    waker: Waker,
}

impl PartialEq for WakeEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline_μs == other.deadline_μs
    }
}
impl Eq for WakeEntry {}

impl PartialOrd for WakeEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WakeEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Natural order so Reverse gives us min-heap behaviour.
        self.deadline_μs.cmp(&other.deadline_μs)
    }
}

// ── SimulatedClock impl ───────────────────────────────────────────────────────

/// Default: 10000 sim-μs per real 10000 μs = 1:1 speed.
const DEFAULT_STEP_MS: u64 = 10000;

impl SimulatedClock {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                now_μs: Arc::new(AtomicU64::new(0)),
                paused: AtomicBool::new(false),
                step_μs: AtomicU64::new(DEFAULT_STEP_MS),
                wakers: Mutex::new(BinaryHeap::new()),
            }),
        }
    }

    /// Current simulated time in microseconds.
    pub fn now_μs(&self) -> u64 {
        self.inner.now_μs.load(Ordering::Relaxed)
    }

    /// Shared reference to the now_μs counter — pass to `Node::new` as
    /// `clock_tick` so message-event timestamps are always current.
    pub fn now_μs_arc(&self) -> Arc<AtomicU64> {
        self.inner.now_μs.clone()
    }

    pub fn pause(&self) {
        self.inner.paused.store(true, Ordering::Relaxed);
    }
    pub fn resume(&self) {
        self.inner.paused.store(false, Ordering::Relaxed);
    }
    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::Relaxed)
    }

    /// Simulated microseconds added per auto-advance real-time tick.
    pub fn speed_μs(&self) -> u64 {
        self.inner.step_μs.load(Ordering::Relaxed)
    }

    /// Set clock speed.  Clamped to [1, 10000] sim-μs per tick.
    pub fn set_speed_μs(&self, μs: u64) {
        self.inner
            .step_μs
            .store(μs.clamp(1, 10_000), Ordering::Relaxed);
    }

    /// Advance the clock by `μs` simulated microseconds and wake all
    /// sleeps whose deadline has now passed.
    pub fn tick(&self, μs: u64) {
        let new_now = self.inner.now_μs.fetch_add(μs, Ordering::Relaxed) + μs;
        let mut heap = self.inner.wakers.lock().unwrap();
        while let Some(Reverse(e)) = heap.peek() {
            if e.deadline_μs <= new_now {
                let Reverse(e) = heap.pop().unwrap();
                e.waker.wake();
            } else {
                break;
            }
        }
    }

    fn register_waker(&self, deadline_μs: u64, waker: Waker) {
        self.inner.wakers.lock().unwrap().push(Reverse(WakeEntry {
            deadline_μs, waker
        }));
    }
}

impl Default for SimulatedClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SimulatedClock {
    fn sleep(&self, duration: Duration) -> SleepFuture {
        let deadline_μs = self.now_μs() + duration.as_micros() as u64;
        Box::pin(SimSleep {
            clock: self.clone(),
            deadline_μs,
        })
    }
}

// ── SimSleep future ───────────────────────────────────────────────────────────

struct SimSleep {
    clock: SimulatedClock,
    deadline_μs: u64,
}

impl Future for SimSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.clock.now_μs() >= self.deadline_μs {
            return Poll::Ready(());
        }
        // Re-register on every poll so the waker is always current.
        self.clock
            .register_waker(self.deadline_μs, cx.waker().clone());
        Poll::Pending
    }
}

// ── Auto-advance task ─────────────────────────────────────────────────────────

/// Advances the clock every `tick_interval_μs` real microseconds by
/// `clock.speed_μs()` simulated microseconds.  The step is read fresh each
/// tick so changing speed takes effect immediately.
pub fn spawn_auto_advance(
    clock: SimulatedClock,
    tick_interval_μs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_micros(tick_interval_μs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if !clock.is_paused() {
                clock.tick(clock.speed_μs());
            }
        }
    })
}
