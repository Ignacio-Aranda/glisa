//! One HID node and the requests going through it. Several devices may live behind it (a
//! receiver).
//!
//! A reader thread takes every report off the transport: the answer to the one request
//! in flight (matched by device index, feature index, function and our software id),
//! and notifications (software id 0), which go to the listeners. Answers to other
//! programs' requests (another software id) are ignored.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::constants::{REPORT_LONG, REPORT_SHORT, REPORT_VERY_LONG, SW_ID, report_len};
use crate::error::{Error, Result};
use crate::transport::{Transport, lock};

/// Default time to wait for an answer.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the reader blocks in one `read` before checking whether it must stop.
const READ_SLICE: Duration = Duration::from_millis(500);
/// Payloads handed out are at least this long (zero padded), so feature parsers can
/// index a long report's 16 bytes without bounds checks failing on short answers.
const MIN_PAYLOAD: usize = 16;

/// `(device index, feature index, function, payload)` of a notification.
pub type NotifyFn = Arc<dyn Fn(u8, u8, u8, &[u8]) + Send + Sync>;
/// Called once when the transport closes by itself (not after [`Channel::close`]).
pub type ClosedFn = Arc<dyn Fn() + Send + Sync>;

/// Handle to remove a listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListenerId(u64);

enum Reply {
    Ok(Vec<u8>),
    Err20(u8),
    Err10(u8),
}

struct Pending {
    dev: u8,
    feat: u8,
    fnsw: u8,
    reply: Option<Reply>,
}

#[derive(Default)]
struct Inner {
    pending: Option<Pending>,
    listeners: Vec<(u64, NotifyFn)>,
    closed_listeners: Vec<(u64, ClosedFn)>,
    next_id: u64,
}

pub struct Channel {
    transport: Arc<dyn Transport>,
    sw_id: u8,
    inner: Mutex<Inner>,
    answered: Condvar,
    req_lock: Mutex<()>,
    closed: AtomicBool,
    stop: Arc<AtomicBool>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Channel {
    /// Starts reading `transport` with our software id ([`SW_ID`]).
    pub fn new(transport: Arc<dyn Transport>) -> Arc<Self> {
        Self::with_sw_id(transport, SW_ID)
    }

    /// Same, with another software id (1..=15), e.g. for a diagnostic tool running next
    /// to the service.
    pub fn with_sw_id(transport: Arc<dyn Transport>, sw_id: u8) -> Arc<Self> {
        let sw_id = match sw_id & 0x0F {
            0 => SW_ID,
            id => id,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let ch = Arc::new(Channel {
            transport: transport.clone(),
            sw_id,
            inner: Mutex::new(Inner::default()),
            answered: Condvar::new(),
            req_lock: Mutex::new(()),
            closed: AtomicBool::new(false),
            stop: stop.clone(),
            reader: Mutex::new(None),
        });
        let weak = Arc::downgrade(&ch);
        let name = format!("hidpp-reader:{}", transport.path());
        let handle = std::thread::Builder::new()
            .name(name)
            .spawn(move || reader_loop(transport, weak, stop))
            .expect("spawn HID++ reader thread");
        *lock(&ch.reader) = Some(handle);
        ch
    }

    pub fn path(&self) -> &str {
        self.transport.path()
    }

    pub fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Whether the reader thread is still running.
    pub fn reader_alive(&self) -> bool {
        lock(&self.reader)
            .as_ref()
            .is_some_and(|h| !h.is_finished())
    }

    /// Waits up to `timeout` for the reader thread to end; true when it has.
    pub fn join_reader(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while self.reader_alive() {
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(h) = lock(&self.reader).take() {
            let _ = h.join();
        }
        true
    }

    pub fn add_listener(&self, f: NotifyFn) -> ListenerId {
        let mut inner = lock(&self.inner);
        inner.next_id += 1;
        let id = inner.next_id;
        inner.listeners.push((id, f));
        ListenerId(id)
    }

    pub fn remove_listener(&self, id: ListenerId) {
        lock(&self.inner).listeners.retain(|(i, _)| *i != id.0);
    }

    pub fn add_closed_listener(&self, f: ClosedFn) -> ListenerId {
        let mut inner = lock(&self.inner);
        inner.next_id += 1;
        let id = inner.next_id;
        inner.closed_listeners.push((id, f));
        ListenerId(id)
    }

    pub fn remove_closed_listener(&self, id: ListenerId) {
        lock(&self.inner)
            .closed_listeners
            .retain(|(i, _)| *i != id.0);
    }

    /// Closes the transport and stops the reader. Closed listeners are not called: they
    /// are for the device going away by itself.
    pub fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.transport.close();
        self.closed.store(true, Ordering::SeqCst);
        let _g = lock(&self.inner);
        self.answered.notify_all();
    }

    fn on_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let listeners: Vec<ClosedFn> = {
            let inner = lock(&self.inner);
            self.answered.notify_all();
            inner
                .closed_listeners
                .iter()
                .map(|(_, f)| f.clone())
                .collect()
        };
        for f in listeners {
            let _ = catch_unwind(AssertUnwindSafe(|| f()));
        }
    }

    fn on_report(&self, data: &[u8]) {
        if data.len() < 4 || !matches!(data[0], REPORT_SHORT | REPORT_LONG | REPORT_VERY_LONG) {
            return;
        }
        let (devidx, sub, addr) = (data[1], data[2], data[3]);
        let b4 = data.get(4).copied().unwrap_or(0);
        let b5 = data.get(5).copied().unwrap_or(0);
        {
            let mut inner = lock(&self.inner);
            if let Some(p) = inner.pending.as_mut().filter(|p| p.reply.is_none()) {
                let reply = if devidx == p.dev && sub == p.feat && addr == p.fnsw {
                    Some(Reply::Ok(payload(&data[4..])))
                } else if devidx == p.dev && sub == 0xFF && addr == p.feat && b4 == p.fnsw {
                    Some(Reply::Err20(b5))
                } else if devidx == p.dev && sub == 0x8F && addr == p.feat && b4 == p.fnsw {
                    Some(Reply::Err10(b5))
                } else {
                    None
                };
                if reply.is_some() {
                    p.reply = reply;
                    self.answered.notify_all();
                    return;
                }
            }
        }
        // Notification (software id 0); answers to other programs are ignored.
        if sub != 0xFF && sub != 0x8F && addr & 0x0F == 0 {
            let listeners: Vec<NotifyFn> = lock(&self.inner)
                .listeners
                .iter()
                .map(|(_, f)| f.clone())
                .collect();
            let body = payload(&data[4..]);
            for f in listeners {
                let _ = catch_unwind(AssertUnwindSafe(|| f(devidx, sub, addr >> 4, &body)));
            }
        }
    }

    /// Sends a request and waits for its answer (the payload after the 4 header bytes,
    /// zero padded to at least 16 bytes).
    pub fn request(
        &self,
        devidx: u8,
        feat_index: u8,
        function: u8,
        params: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        if self.is_closed() {
            return Err(Error::NoResponse("channel closed".into()));
        }
        let mut report_id = if self.transport.supports_long() || params.len() > 3 {
            REPORT_LONG
        } else {
            REPORT_SHORT
        };
        if !self.transport.supports_long() {
            report_id = REPORT_SHORT;
        }
        if !self.transport.supports_short() {
            report_id = REPORT_LONG;
        }
        let size = report_len(report_id).unwrap_or(20);
        let fnsw = ((function & 0x0F) << 4) | self.sw_id;
        let mut msg = vec![report_id, devidx, feat_index, fnsw];
        msg.extend_from_slice(params);
        msg.resize(size, 0);

        let _serial = lock(&self.req_lock);
        lock(&self.inner).pending = Some(Pending {
            dev: devidx,
            feat: feat_index,
            fnsw,
            reply: None,
        });
        let reply = match self.transport.write(&msg) {
            Err(e) => {
                lock(&self.inner).pending = None;
                return Err(Error::Closed(e));
            }
            Ok(()) => {
                let deadline = Instant::now() + timeout;
                let mut inner = lock(&self.inner);
                loop {
                    let done = inner.pending.as_ref().is_some_and(|p| p.reply.is_some());
                    let now = Instant::now();
                    if done || self.is_closed() || now >= deadline {
                        break;
                    }
                    inner = self
                        .answered
                        .wait_timeout(inner, deadline - now)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
                inner.pending.take().and_then(|p| p.reply)
            }
        };
        match reply {
            Some(Reply::Ok(payload)) => Ok(payload),
            Some(Reply::Err20(code)) => Err(Error::hidpp20(code)),
            Some(Reply::Err10(code)) => Err(Error::hidpp10(code)),
            None => Err(Error::NoResponse(format!(
                "no response dev={devidx:#x} feat={feat_index} fn={function}"
            ))),
        }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn payload(body: &[u8]) -> Vec<u8> {
    let mut v = body.to_vec();
    if v.len() < MIN_PAYLOAD {
        v.resize(MIN_PAYLOAD, 0);
    }
    v
}

fn reader_loop(transport: Arc<dyn Transport>, channel: Weak<Channel>, stop: Arc<AtomicBool>) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match transport.read(READ_SLICE) {
            Ok(Some(data)) => match channel.upgrade() {
                Some(ch) => ch.on_report(&data),
                None => return,
            },
            Ok(None) => {
                if channel.strong_count() == 0 {
                    return;
                }
            }
            Err(_) => break,
        }
    }
    if !stop.load(Ordering::SeqCst) {
        if let Some(ch) = channel.upgrade() {
            ch.on_closed();
        }
    }
}
