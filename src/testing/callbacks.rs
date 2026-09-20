use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Counts how many times something ran, across threads.
///
/// Cloning shares the count, so a `Recorder` can be handed to a callback and still read from the
/// test.
#[derive(Debug, Clone, Default)]
pub struct Recorder {
    count: Arc<AtomicUsize>,
}

impl Recorder {
    /// A recorder at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one occurrence.
    pub fn hit(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    /// How many occurrences have been recorded.
    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Block until the count reaches `target`, or `timeout` passes.
    ///
    /// Returns whether it got there.
    pub fn wait_for(&self, target: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.count() < target {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }
}

/// A gate a callback can be parked on, so a test can hold a dispatch open while something else
/// happens.
///
/// This is how you make a stop overlap a running callback on purpose: register a callback that
/// waits on a `Barrier`, fire the hook on another thread, wait until it is inside, then stop the
/// plugin and check that the stopping thread blocks until the barrier opens.
#[derive(Debug, Clone, Default)]
pub struct Barrier {
    state: Arc<GateState>,
}

#[derive(Debug, Default)]
struct GateState {
    open: Mutex<bool>,
    changed: Condvar,
    arrived: AtomicUsize,
}

impl Barrier {
    /// A closed barrier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Block until the barrier is opened. Records an arrival first, so a test can tell that
    /// something reached the gate before it blocked.
    pub fn wait(&self) {
        self.state.arrived.fetch_add(1, Ordering::SeqCst);
        let mut open = match self.state.open.lock() {
            Ok(open) => open,
            Err(poisoned) => poisoned.into_inner(),
        };
        while !*open {
            open = match self.state.changed.wait(open) {
                Ok(open) => open,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
    }

    /// Let everyone waiting through, and everyone who arrives later.
    pub fn open(&self) {
        let mut open = match self.state.open.lock() {
            Ok(open) => open,
            Err(poisoned) => poisoned.into_inner(),
        };
        *open = true;
        self.state.changed.notify_all();
    }

    /// How many callers have reached [`wait`](Self::wait).
    pub fn arrived(&self) -> usize {
        self.state.arrived.load(Ordering::SeqCst)
    }

    /// Block until `count` callers have reached the gate, or `timeout` passes.
    ///
    /// Returns whether they did.
    pub fn wait_for_arrivals(&self, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.arrived() < count {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }
}
