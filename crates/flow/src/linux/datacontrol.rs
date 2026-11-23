//! The whole clipboard on KDE Wayland through the Wayland data-control protocol.
//!
//! A background process can't use a toolkit clipboard on Wayland (an unfocused client sees
//! no clipboard), and klipper's D-Bus API only knows text. KWin advertises data-control for
//! clipboard managers instead — `ext_data_control_manager_v1` (KWin ≥ 6.3; verified on
//! Plasma 6.6) or the older `zwlr_data_control_manager_v1`, with the same requests and
//! events — which lets a client list the MIME types of the current selection, read any of
//! them through a pipe, and set a selection of its own (served from memory or from a staged
//! file when someone pastes). Clipboard contents are never logged.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::wayland::{self, DISPLAY, Message, Sender, Wire, string, u32s};
use crate::clipboard::{RichClipboard, TooLarge, copy_out};
use crate::transfer::{ClipData, Offers};
use crate::util::lock;

/// Preferred first.
pub const MANAGERS: [&str; 2] = [
    "ext_data_control_manager_v1",
    "zwlr_data_control_manager_v1",
];
const REGISTRY: u32 = 2;
// requests
const M_CREATE_SOURCE: u16 = 0;
const M_GET_DEVICE: u16 = 1;
const DEV_SET_SELECTION: u16 = 0;
const SRC_OFFER: u16 = 0;
const SRC_DESTROY: u16 = 1;
const OFFER_RECEIVE: u16 = 0;
const OFFER_DESTROY: u16 = 1;
// events
const EV_DEV_OFFER: u16 = 0;
const EV_DEV_SELECTION: u16 = 1;
const EV_DEV_FINISHED: u16 = 2;
const EV_DEV_PRIMARY: u16 = 3;
const EV_OFFER_MIME: u16 = 0;
const EV_SRC_SEND: u16 = 0;
const EV_SRC_CANCELLED: u16 = 1;
const EV_CALLBACK_DONE: u16 = 0;
/// Someone pasting has this long to read what we serve.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

struct State {
    alive: bool,
    next_id: u32,
    manager: u32,
    device: u32,
    offers: HashMap<u32, Vec<String>>,
    selection: Option<u32>,
    sources: HashMap<u32, Offers>,
    callbacks: HashMap<u32, bool>,
}

pub struct DataControl {
    st: Arc<(Mutex<State>, Condvar)>,
    sender: Mutex<Option<Arc<Sender>>>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    pub manager_name: Mutex<String>,
    pub error: Arc<Mutex<String>>,
}

impl Default for DataControl {
    fn default() -> Self {
        Self::new()
    }
}

impl DataControl {
    pub fn new() -> Self {
        Self {
            st: Arc::new((
                Mutex::new(State {
                    alive: false,
                    next_id: 3,
                    manager: 0,
                    device: 0,
                    offers: HashMap::new(),
                    selection: None,
                    sources: HashMap::new(),
                    callbacks: HashMap::new(),
                }),
                Condvar::new(),
            )),
            sender: Mutex::new(None),
            stop: Arc::default(),
            thread: Mutex::new(None),
            manager_name: Mutex::new(String::new()),
            error: Arc::default(),
        }
    }

    fn new_id(st: &mut State) -> u32 {
        let id = st.next_id;
        st.next_id += 1;
        id
    }

    fn sender(&self) -> Option<Arc<Sender>> {
        lock(&self.sender).clone()
    }

    /// True when the compositor offers the protocol (else `error` says why). Connects to
    /// `sock`, or to the session's compositor.
    pub fn start(&self, sock: Option<UnixStream>) -> bool {
        let timeout = Duration::from_secs(2);
        let result = (|| -> std::io::Result<Option<Wire>> {
            let sock = match sock {
                Some(s) => s,
                None => wayland::connect(timeout)?,
            };
            let mut wire = Wire::new(sock);
            let sync = Self::new_id(&mut lock(&self.st.0));
            let globals = wayland::globals(&mut wire, REGISTRY, sync, timeout)?;
            let Some(manager) = MANAGERS.iter().find(|m| globals.contains_key(**m)) else {
                return Ok(None);
            };
            let Some((seat_name, _)) = globals.get("wl_seat") else {
                return Ok(None);
            };
            let (mname, _) = globals[*manager];
            *lock(&self.manager_name) = manager.to_string();
            let (seat, mgr, dev) = {
                let mut st = lock(&self.st.0);
                let ids = (
                    Self::new_id(&mut st),
                    Self::new_id(&mut st),
                    Self::new_id(&mut st),
                );
                st.manager = ids.1;
                st.device = ids.2;
                st.alive = true;
                ids
            };
            wayland::bind(&wire, REGISTRY, *seat_name, "wl_seat", 1, seat)?;
            wayland::bind(&wire, REGISTRY, mname, manager, 1, mgr)?;
            wire.send(mgr, M_GET_DEVICE, &u32s(&[dev, seat]))?;
            Ok(Some(wire))
        })();
        let wire = match result {
            Ok(Some(w)) => w,
            Ok(None) => {
                *lock(&self.error) =
                    "the compositor doesn't offer the data-control protocol".into();
                return false;
            }
            Err(e) => {
                *lock(&self.error) = e.to_string();
                lock(&self.st.0).alive = false;
                return false;
            }
        };
        let Ok(sender) = wire.sender() else {
            lock(&self.st.0).alive = false;
            return false;
        };
        *lock(&self.sender) = Some(sender.clone());
        self.stop.store(false, Ordering::SeqCst);
        let (st, stop, error) = (self.st.clone(), self.stop.clone(), self.error.clone());
        let t = std::thread::Builder::new()
            .name("flow-clipboard".into())
            .spawn(move || run(wire, sender, st, stop, error));
        *lock(&self.thread) = t.ok();
        self.sync(timeout); // the current selection is known when start() returns
        true
    }

    /// Round trip: every event sent before now has been handled.
    pub fn sync(&self, timeout: Duration) -> bool {
        let Some(sender) = self.sender() else {
            return false;
        };
        let (m, cv) = &*self.st;
        let mut st = lock(m);
        if !st.alive {
            return false;
        }
        let cb = Self::new_id(&mut st);
        st.callbacks.insert(cb, false);
        if sender.send(DISPLAY, 0, &u32s(&[cb])).is_err() {
            st.callbacks.remove(&cb);
            return false;
        }
        let end = Instant::now() + timeout;
        while st.alive && !st.callbacks.get(&cb).copied().unwrap_or(true) {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            st = cv
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let done = st.callbacks.remove(&cb).unwrap_or(false);
        done && st.alive
    }

    /// Asks the current selection for `mime`: the read end of the pipe it writes into.
    fn receive(&self, mime: &str) -> Option<OwnedFd> {
        let sender = self.sender()?;
        let st = lock(&self.st.0); // the offer can't be destroyed while we ask it
        let oid = st.selection?;
        if !st.alive
            || !st
                .offers
                .get(&oid)
                .is_some_and(|m| m.iter().any(|x| x == mime))
        {
            return None;
        }
        let (r, w) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).ok()?;
        sender
            .send_fd(oid, OFFER_RECEIVE, &string(mime), w.as_fd())
            .ok()?;
        drop(w);
        Some(r)
    }

    /// Text for the text clipboard (when klipper isn't there).
    pub fn text(&self, timeout: Duration) -> Option<String> {
        let formats = self.formats(timeout)?;
        let mime = crate::transfer::pick_text_type(&formats)?;
        let data = self
            .read(mime, crate::CLIPBOARD_MAX, Duration::from_secs(1))
            .ok()??;
        String::from_utf8(data).ok()
    }

    pub fn set_text(&self, text: &str) -> bool {
        let mut o = Offers::new();
        for mime in crate::transfer::TEXT_TYPES {
            o.insert(mime.to_string(), ClipData::Bytes(text.as_bytes().to_vec()));
        }
        self.put(&o)
    }
}

impl RichClipboard for DataControl {
    fn kind(&self) -> &'static str {
        "data-control"
    }

    fn available(&self) -> bool {
        lock(&self.st.0).alive
    }

    fn formats(&self, timeout: Duration) -> Option<Vec<String>> {
        if !self.sync(timeout) {
            return None;
        }
        let st = lock(&self.st.0);
        Some(
            st.selection
                .and_then(|s| st.offers.get(&s).cloned())
                .unwrap_or_default(),
        )
    }

    fn read_to(
        &self,
        mime: &str,
        out: &mut dyn Write,
        limit: usize,
        timeout: Duration,
    ) -> Result<Option<u64>, TooLarge> {
        let Some(r) = self.receive(mime) else {
            return Ok(None);
        };
        let mut f = std::fs::File::from(r);
        let end = Instant::now() + timeout;
        let mut total = 0usize;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            let ready = {
                use rustix::event::{PollFd, PollFlags, Timespec, poll};
                let fd = f.as_fd();
                let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                let ts = Timespec {
                    tv_sec: left.as_secs() as _,
                    tv_nsec: left.subsec_nanos() as _,
                };
                poll(&mut fds, Some(&ts)).unwrap_or(0) > 0
            };
            if !ready {
                return Ok(None);
            }
            let n = match f.read(&mut buf) {
                Ok(n) => n,
                Err(_) => return Ok(None),
            };
            if n == 0 {
                return Ok(Some(total as u64));
            }
            total += n;
            if total > limit {
                return Err(TooLarge);
            }
            if out.write_all(&buf[..n]).is_err() {
                return Ok(None);
            }
        }
    }

    fn put(&self, offers: &Offers) -> bool {
        if offers.is_empty() {
            return false;
        }
        let Some(sender) = self.sender() else {
            return false;
        };
        let sid = {
            let mut st = lock(&self.st.0);
            if !st.alive {
                return false;
            }
            let sid = Self::new_id(&mut st);
            let (mgr, dev) = (st.manager, st.device);
            let sent = (|| -> std::io::Result<()> {
                sender.send(mgr, M_CREATE_SOURCE, &u32s(&[sid]))?;
                for mime in offers.keys() {
                    sender.send(sid, SRC_OFFER, &string(mime))?;
                }
                Ok(())
            })();
            if sent.is_err() {
                return false;
            }
            st.sources.insert(sid, offers.clone());
            if sender.send(dev, DEV_SET_SELECTION, &u32s(&[sid])).is_err() {
                return false;
            }
            sid
        };
        self.sync(Duration::from_secs(1));
        let st = lock(&self.st.0);
        st.alive && st.sources.contains_key(&sid)
    }

    fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(s) = lock(&self.sender).take() {
            s.shutdown(); // wakes the event thread
        }
        if let Some(t) = lock(&self.thread).take() {
            if t.thread().id() != std::thread::current().id() {
                let _ = t.join();
            }
        }
        lock(&self.st.0).alive = false;
        self.st.1.notify_all();
    }
}

fn run(
    mut wire: Wire,
    sender: Arc<Sender>,
    st: Arc<(Mutex<State>, Condvar)>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<String>>,
) {
    let result = (|| -> std::io::Result<()> {
        while !stop.load(Ordering::SeqCst) {
            for m in wire.read(Duration::from_millis(250))? {
                dispatch(&mut wire, &sender, &st, m)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        if !stop.load(Ordering::SeqCst) {
            log::info!("Flow: clipboard (data-control) stopped: {e}");
            *lock(&error) = e.to_string();
        }
    }
    let (m, cv) = &*st;
    lock(m).alive = false;
    cv.notify_all();
    wire.fds.clear();
}

fn dispatch(
    wire: &mut Wire,
    sender: &Sender,
    st: &(Mutex<State>, Condvar),
    m: Message,
) -> std::io::Result<()> {
    let (lockm, cv) = st;
    let mut s = lock(lockm);
    let r = |off| wayland::read_u32(&m.body, off);
    if m.object == s.device {
        match m.opcode {
            EV_DEV_OFFER => {
                s.offers.insert(r(0)?, Vec::new());
            }
            EV_DEV_SELECTION => {
                let oid = r(0)?;
                let new = (oid != 0).then_some(oid);
                let old = std::mem::replace(&mut s.selection, new);
                if let Some(old) = old.filter(|o| Some(*o) != new) {
                    drop_offer(sender, &mut s, old);
                }
            }
            EV_DEV_PRIMARY => {
                let oid = r(0)?;
                if oid != 0 && Some(oid) != s.selection {
                    drop_offer(sender, &mut s, oid); // the primary selection is not ours to care about
                }
            }
            EV_DEV_FINISHED => return Err(wayland::bad("the clipboard device is gone")),
            _ => {}
        }
    } else if s.offers.contains_key(&m.object) && m.opcode == EV_OFFER_MIME {
        let (mime, _) = wayland::read_string(&m.body, 0)?;
        let list = s.offers.get_mut(&m.object).expect("offer");
        if list.len() < 256 && mime.len() <= 255 {
            list.push(mime);
        }
    } else if s.sources.contains_key(&m.object) {
        match m.opcode {
            EV_SRC_SEND => {
                let (mime, _) = wayland::read_string(&m.body, 0)?;
                let fd = wire.fds.pop_front();
                let data = s.sources.get(&m.object).and_then(|o| o.get(&mime)).cloned();
                if let (Some(fd), Some(data)) = (fd, data) {
                    std::thread::spawn(move || serve(fd, data));
                }
            }
            EV_SRC_CANCELLED => {
                s.sources.remove(&m.object);
                let _ = sender.send(m.object, SRC_DESTROY, &[]);
            }
            _ => {}
        }
    } else if s.callbacks.contains_key(&m.object) && m.opcode == EV_CALLBACK_DONE {
        s.callbacks.insert(m.object, true);
        cv.notify_all();
    } else if m.object == DISPLAY && m.opcode == 0 {
        return Err(wayland::bad("Wayland protocol error"));
    }
    Ok(())
}

fn drop_offer(sender: &Sender, s: &mut State, oid: u32) {
    if s.offers.remove(&oid).is_some() {
        let _ = sender.send(oid, OFFER_DESTROY, &[]);
    }
}

/// Someone pastes: write our data into their pipe (never blocks the event loop).
fn serve(fd: OwnedFd, data: ClipData) {
    let _ = rustix::fs::fcntl_setfl(&fd, rustix::fs::OFlags::NONBLOCK);
    let mut f = std::fs::File::from(fd);
    let end = Instant::now() + WRITE_TIMEOUT;
    let result = copy_out(&data, &mut |chunk| {
        let mut chunk = chunk;
        while !chunk.is_empty() {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "nobody reads the clipboard",
                ));
            }
            use rustix::event::{PollFd, PollFlags, Timespec, poll};
            let ready = {
                let bfd = f.as_fd();
                let mut fds = [PollFd::new(&bfd, PollFlags::OUT)];
                let ts = Timespec {
                    tv_sec: left.as_secs() as _,
                    tv_nsec: left.subsec_nanos() as _,
                };
                poll(&mut fds, Some(&ts))? > 0
            };
            if !ready {
                continue;
            }
            match f.write(chunk) {
                Ok(n) => chunk = &chunk[n..],
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    });
    if let Err(e) = result {
        log::debug!("Flow: clipboard data not delivered: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::keystate::tests::msg;
    use std::collections::BTreeMap;
    use std::os::fd::AsFd;

    /// Plays KWin's ext_data_control_v1 on the other end of a socketpair: the registry,
    /// one data device, offers, selections and fd passing.
    struct Compositor {
        wire: Wire,
        selection: BTreeMap<String, Vec<u8>>,
        objects: HashMap<u32, &'static str>,
        source_mimes: HashMap<u32, Vec<String>>,
        client_source: Option<u32>,
        offers: Vec<u32>,
        next_server_id: u32,
        device: Option<u32>,
    }

    impl Compositor {
        fn send(&self, obj: u32, op: u16, payload: &[u8]) {
            let _ = (&self.wire.sock).write_all(&msg(obj, op, payload));
        }

        fn announce(&mut self) {
            let Some(dev) = self.device else { return };
            let mimes: Vec<String> = match self.client_source {
                Some(s) => self.source_mimes[&s].clone(),
                None => self.selection.keys().cloned().collect(),
            };
            if mimes.is_empty() {
                self.send(dev, 1, &0u32.to_ne_bytes());
                return;
            }
            let oid = self.next_server_id;
            self.next_server_id += 1;
            self.offers.push(oid);
            self.send(dev, 0, &oid.to_ne_bytes());
            for m in mimes {
                self.send(oid, 0, &string(&m));
            }
            self.send(dev, 1, &oid.to_ne_bytes());
        }

        fn copy_elsewhere(&mut self, data: BTreeMap<String, Vec<u8>>) {
            if let Some(s) = self.client_source.take() {
                self.send(s, 1, &[]); // cancelled
            }
            self.selection = data;
            self.announce();
        }

        fn request(&mut self, m: Message) {
            let kind = self
                .objects
                .get(&m.object)
                .copied()
                .unwrap_or(if m.object == 1 { "display" } else { "" });
            let r = |off| wayland::read_u32(&m.body, off).unwrap();
            match (kind, m.opcode) {
                ("display", 1) => {
                    let reg = r(0);
                    self.objects.insert(reg, "registry");
                    self.send(
                        reg,
                        0,
                        &[
                            &7u32.to_ne_bytes()[..],
                            &string("wl_seat"),
                            &9u32.to_ne_bytes(),
                        ]
                        .concat(),
                    );
                    self.send(
                        reg,
                        0,
                        &[
                            &15u32.to_ne_bytes()[..],
                            &string("ext_data_control_manager_v1"),
                            &1u32.to_ne_bytes(),
                        ]
                        .concat(),
                    );
                }
                ("display", 0) => self.send(r(0), 0, &1u32.to_ne_bytes()),
                ("registry", 0) => {
                    let (iface, off) = wayland::read_string(&m.body, 4).unwrap();
                    let new = wayland::read_u32(&m.body, off + 4).unwrap();
                    let k: &'static str = if iface == "wl_seat" {
                        "seat"
                    } else {
                        "manager"
                    };
                    self.objects.insert(new, k);
                }
                ("manager", 1) => {
                    self.device = Some(r(0));
                    self.objects.insert(r(0), "device");
                    self.announce();
                }
                ("manager", 0) => {
                    self.objects.insert(r(0), "source");
                    self.source_mimes.insert(r(0), Vec::new());
                }
                ("source", 0) => {
                    let mime = wayland::read_string(&m.body, 0).unwrap().0;
                    self.source_mimes.get_mut(&m.object).unwrap().push(mime);
                }
                ("device", 0) => {
                    let sid = r(0);
                    if let Some(old) = self.client_source.filter(|s| *s != sid) {
                        self.send(old, 1, &[]);
                    }
                    self.client_source = Some(sid);
                    self.announce();
                }
                _ if self.offers.contains(&m.object) && m.opcode == 0 => {
                    let mime = wayland::read_string(&m.body, 0).unwrap().0;
                    let fd = self.wire.fds.pop_front().unwrap();
                    if let Some(s) = self.client_source {
                        let sender = self.wire.sender().unwrap();
                        sender.send_fd(s, 0, &string(&mime), fd.as_fd()).unwrap();
                    } else {
                        let data = self.selection.get(&mime).cloned().unwrap_or_default();
                        std::thread::spawn(move || {
                            let mut f = std::fs::File::from(fd);
                            let _ = f.write_all(&data);
                        });
                    }
                }
                _ if self.offers.contains(&m.object) && m.opcode == 1 => {
                    self.offers.retain(|o| *o != m.object)
                }
                _ => {}
            }
        }
    }

    enum Cmd {
        CopyElsewhere(BTreeMap<String, Vec<u8>>),
    }

    fn start_compositor(
        sock: UnixStream,
        selection: BTreeMap<String, Vec<u8>>,
        with_protocol: bool,
    ) -> std::sync::mpsc::Sender<Cmd> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut c = Compositor {
                wire: Wire::new(sock),
                selection,
                objects: HashMap::new(),
                source_mimes: HashMap::new(),
                client_source: None,
                offers: Vec::new(),
                next_server_id: 0xFF00_0000,
                device: None,
            };
            loop {
                while let Ok(Cmd::CopyElsewhere(d)) = rx.try_recv() {
                    c.copy_elsewhere(d);
                }
                let Ok(msgs) = c.wire.read(Duration::from_millis(20)) else {
                    return;
                };
                for m in msgs {
                    if !with_protocol && m.object == 1 && m.opcode == 1 {
                        let reg = wayland::read_u32(&m.body, 0).unwrap();
                        c.send(
                            reg,
                            0,
                            &[
                                &7u32.to_ne_bytes()[..],
                                &string("wl_seat"),
                                &9u32.to_ne_bytes(),
                            ]
                            .concat(),
                        );
                        continue;
                    }
                    c.request(m);
                }
            }
        });
        tx
    }

    fn wait(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        f()
    }

    #[test]
    fn read_put_and_serve() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut sel = BTreeMap::new();
        sel.insert("text/plain".to_string(), b"/tmp/x".to_vec());
        sel.insert("text/uri-list".to_string(), b"file:///tmp/x\r\n".to_vec());
        let comp = start_compositor(b, sel, true);
        let dc = DataControl::new();
        assert!(dc.start(Some(a)), "{}", dc.error.lock().unwrap());
        let t = Duration::from_secs(2);
        assert_eq!(
            *dc.manager_name.lock().unwrap(),
            "ext_data_control_manager_v1"
        );
        assert_eq!(dc.formats(t).unwrap(), vec!["text/plain", "text/uri-list"]);
        assert_eq!(
            dc.read("text/uri-list", 1024, t).unwrap().unwrap(),
            b"file:///tmp/x\r\n"
        );
        assert_eq!(dc.read("image/png", 1024, t).unwrap(), None);
        assert_eq!(dc.read("text/uri-list", 4, t), Err(TooLarge));
        assert_eq!(dc.text(t).as_deref(), Some("/tmp/x"));
        let png = crate::imageinfo::tests::png_bytes(5, 5, b"");
        let mut d = BTreeMap::new();
        d.insert("image/png".to_string(), png.clone());
        comp.send(Cmd::CopyElsewhere(d)).unwrap();
        assert!(wait(|| dc.formats(t) == Some(vec!["image/png".to_string()])));
        let tmp = tempfile::tempdir().unwrap();
        let img = tmp.path().join("img");
        std::fs::write(&img, &png).unwrap();
        let mut offers = Offers::new();
        offers.insert("image/png".into(), ClipData::File(img));
        offers.insert(
            "text/plain;charset=utf-8".into(),
            ClipData::Bytes(b"pie".to_vec()),
        );
        assert!(dc.put(&offers));
        assert_eq!(
            dc.formats(t).unwrap(),
            vec!["image/png", "text/plain;charset=utf-8"]
        );
        assert_eq!(dc.read("image/png", 1 << 20, t).unwrap().unwrap(), png); // served by our own source
        assert_eq!(
            dc.read("text/plain;charset=utf-8", 100, t)
                .unwrap()
                .unwrap(),
            b"pie"
        );
        let mut d = BTreeMap::new();
        d.insert("text/plain".to_string(), b"otra".to_vec());
        comp.send(Cmd::CopyElsewhere(d)).unwrap(); // our source is cancelled
        assert!(wait(|| dc.st.0.lock().unwrap().sources.is_empty()));
        assert_eq!(dc.formats(t).unwrap(), vec!["text/plain"]);
        assert!(dc.set_text("hola"));
        assert_eq!(dc.text(t).as_deref(), Some("hola"));
        dc.close();
        assert!(!dc.available());
    }

    #[test]
    fn without_the_protocol() {
        let (a, b) = UnixStream::pair().unwrap();
        start_compositor(b, BTreeMap::new(), false);
        let dc = DataControl::new();
        assert!(!dc.start(Some(a)));
        assert!(dc.error.lock().unwrap().contains("data-control"));
        assert!(!dc.available());
    }
}
