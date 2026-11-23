//! Byte-level duplex channel carrying HID++ reports (report id first), implemented by the
//! OS transports in `glisa-hid` and by the simulator.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

pub use crate::error::TransportClosed;

pub trait Transport: Send + Sync {
    /// OS path (hidraw node, HID path) or `sim://<profile>`.
    fn path(&self) -> &str;
    fn supports_short(&self) -> bool {
        true
    }
    fn supports_long(&self) -> bool {
        true
    }
    /// Sends one report (id first, exact report length).
    fn write(&self, data: &[u8]) -> Result<(), TransportClosed>;
    /// One report, or `None` on timeout.
    fn read(&self, timeout: Duration) -> Result<Option<Vec<u8>>, TransportClosed>;
    /// Stops the transport: a blocked or later `read` fails with [`TransportClosed`].
    fn close(&self);
}

/// A thread-safe FIFO of reports with a blocking, timed `pop`: the simulator's output.
#[derive(Default)]
pub struct ReportQueue {
    items: Mutex<VecDeque<Vec<u8>>>,
    ready: Condvar,
}

impl ReportQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, report: Vec<u8>) {
        lock(&self.items).push_back(report);
        self.ready.notify_all();
    }

    /// Next report, waiting up to `timeout`; `None` when nothing came.
    pub fn pop(&self, timeout: Duration) -> Option<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut items = lock(&self.items);
        loop {
            if let Some(r) = items.pop_front() {
                return Some(r);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            items = self
                .ready
                .wait_timeout(items, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Wakes every `pop` (they return what is queued or time out normally).
    pub fn wake(&self) {
        self.ready.notify_all();
    }

    pub fn len(&self) -> usize {
        lock(&self.items).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Locks a mutex, ignoring poisoning (a panicking listener must not wedge a device).
pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
