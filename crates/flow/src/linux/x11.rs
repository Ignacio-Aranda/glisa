//! Any X11 session: the pointer polled at 60 Hz (`QueryPointer`, which also gives the Ctrl
//! and mouse button state), `WarpPointer` for placement, RandR monitors for the geometry,
//! and the CLIPBOARD selection for text, copied files (`text/uri-list`) and images
//! (`image/png`), read and served with the INCR protocol for large data.
//!
//! Edges are only reported when the pointer moves, never while a mouse button is held
//! (dragging a window or a file to the edge is not a switch; releasing it there isn't either,
//! until the pointer pushes the edge again), as the KWin script does.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glisa_i18n::n_;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, SelectionNotifyEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::backends::{Base, EdgeBackend, Sink, report};
use crate::clipboard::{RichClipboard, TooLarge, copy_out};
use crate::geometry::{EdgeTracker, Rect, clean_rects, now_s};
use crate::text::Text;
use crate::transfer::{ClipData, Offers};
use crate::util::lock;

const CONTROL_MASK: u16 = 1 << 2;
const BUTTONS_MASK: u16 = 0x1F00; // Button1Mask .. Button5Mask
const POLL: Duration = Duration::from_millis(16);

/// Where the pointer is (the X server; a fake in tests).
pub trait PointerSource: Send {
    /// (x, y, modifier/button mask)
    fn pointer(&mut self) -> Option<(i64, i64, u16)>;
    fn screens(&mut self) -> Vec<Rect>;
    fn warp(&mut self, x: i64, y: i64);
}

struct XPointer {
    conn: RustConnection,
    root: Window,
    width: i64,
    height: i64,
}

impl XPointer {
    fn open() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let s = &conn.setup().roots[screen];
        let (root, width, height) = (s.root, s.width_in_pixels as i64, s.height_in_pixels as i64);
        Ok(Self {
            conn,
            root,
            width,
            height,
        })
    }
}

impl PointerSource for XPointer {
    fn pointer(&mut self) -> Option<(i64, i64, u16)> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((r.root_x as i64, r.root_y as i64, u16::from(r.mask)))
    }

    fn screens(&mut self) -> Vec<Rect> {
        let monitors = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| {
                r.monitors
                    .iter()
                    .map(|m| [m.x as i64, m.y as i64, m.width as i64, m.height as i64])
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let rects = clean_rects(&monitors);
        if rects.is_empty() {
            vec![[0, 0, self.width, self.height]]
        } else {
            rects
        }
    }

    fn warp(&mut self, x: i64, y: i64) {
        let _ = self
            .conn
            .warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, x as i16, y as i16);
        let _ = self.conn.flush();
    }
}

pub type SourceFactory = Box<dyn Fn() -> Result<Box<dyn PointerSource>, String> + Send + Sync>;

struct Shared {
    base: Base,
    screens: Mutex<Vec<Rect>>,
    ctrl: Mutex<Option<bool>>,
    buttons: Mutex<Option<bool>>,
    warp: Mutex<Option<(i64, i64)>>,
    stop: AtomicBool,
}

pub struct X11Backend {
    sh: Arc<Shared>,
    factory: Arc<SourceFactory>,
    thread: Mutex<Option<JoinHandle<()>>>,
    clipboard: Mutex<Option<Arc<X11Clipboard>>>,
    with_clipboard: bool,
}

impl Default for X11Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl X11Backend {
    pub fn new() -> Self {
        Self::with_source(
            Box::new(|| XPointer::open().map(|p| Box::new(p) as Box<dyn PointerSource>)),
            true,
        )
    }

    pub fn with_source(factory: SourceFactory, clipboard: bool) -> Self {
        Self {
            sh: Arc::new(Shared {
                base: Base::new(),
                screens: Mutex::new(Vec::new()),
                ctrl: Mutex::new(None),
                buttons: Mutex::new(None),
                warp: Mutex::new(None),
                stop: AtomicBool::new(false),
            }),
            factory: Arc::new(factory),
            thread: Mutex::new(None),
            clipboard: Mutex::new(None),
            with_clipboard: clipboard,
        }
    }

    fn run(sh: Arc<Shared>, factory: Arc<SourceFactory>) {
        let mut src = match factory() {
            Ok(s) => s,
            Err(reason) => {
                log::info!("Flow: X11 unavailable: {reason}");
                sh.base.set_state(
                    "error",
                    Some(
                        Text::new(n_!("Flow is not available on this desktop: {reason}"))
                            .arg("reason", reason),
                    ),
                );
                return;
            }
        };
        sh.base.set_state("ok", None);
        let mut tracker = EdgeTracker::new(32.0);
        let mut last_screens: Option<Instant> = None;
        let mut last_pos: Option<(i64, i64)> = None;
        while !sh.stop.load(Ordering::SeqCst) {
            if last_screens.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                let rects = src.screens();
                if *lock(&sh.screens) != rects {
                    *lock(&sh.screens) = rects.clone();
                    if let Some(s) = sh.base.sink() {
                        s.screens(rects);
                    }
                }
                last_screens = Some(Instant::now());
            }
            if let Some((x, y)) = lock(&sh.warp).take() {
                src.warp(x, y);
            }
            if let Some((x, y, mask)) = src.pointer() {
                let ctrl = mask & CONTROL_MASK != 0;
                let changed = lock(&sh.ctrl).replace(ctrl) != Some(ctrl);
                if changed {
                    if let Some(s) = sh.base.sink() {
                        s.ctrl(ctrl);
                    }
                }
                let held = mask & BUTTONS_MASK != 0;
                *lock(&sh.buttons) = Some(held);
                // Like the KWin script: only movements count (releasing a button at the edge
                // is not a push against it).
                if last_pos != Some((x, y)) {
                    last_pos = Some((x, y));
                    let rects = lock(&sh.screens).clone();
                    let events = tracker.feed(x as f64, y as f64, &rects, now_s(), held);
                    if let Some(s) = sh.base.sink() {
                        report(s.as_ref(), events);
                    }
                }
            }
            std::thread::sleep(POLL);
        }
    }
}

impl EdgeBackend for X11Backend {
    fn kind(&self) -> &'static str {
        "x11"
    }

    fn start(&self, sink: Arc<dyn Sink>) {
        self.sh.base.set_sink(Some(sink));
        self.sh.base.set_state("starting", None);
        self.sh.stop.store(false, Ordering::SeqCst);
        let (sh, f) = (self.sh.clone(), self.factory.clone());
        *lock(&self.thread) = std::thread::Builder::new()
            .name("flow-x11".into())
            .spawn(move || Self::run(sh, f))
            .ok();
        if self.with_clipboard {
            match X11Clipboard::start() {
                Ok(c) => *lock(&self.clipboard) = Some(c),
                Err(e) => log::info!("Flow: the X11 clipboard isn't available: {e}"),
            }
        }
    }

    fn stop(&self) {
        self.sh.stop.store(true, Ordering::SeqCst);
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
        if let Some(c) = lock(&self.clipboard).take() {
            c.close();
        }
        self.sh.base.set_state("off", None);
        self.sh.base.set_sink(None);
    }

    fn state(&self) -> (String, Option<Text>) {
        self.sh.base.get_state()
    }

    fn screens(&self) -> Vec<Rect> {
        lock(&self.sh.screens).clone()
    }

    fn ctrl_held(&self) -> Option<bool> {
        *lock(&self.sh.ctrl)
    }

    fn ctrl_available(&self) -> bool {
        self.sh.base.get_state().0 == "ok"
    }

    fn buttons_held(&self) -> Option<bool> {
        *lock(&self.sh.buttons)
    }

    fn clipboard_kind(&self) -> &'static str {
        if lock(&self.clipboard)
            .as_ref()
            .is_some_and(|c| c.available())
        {
            "x11"
        } else {
            "none"
        }
    }

    fn clipboard_get(&self, timeout: Duration) -> Option<String> {
        let c = lock(&self.clipboard).clone()?;
        let formats = c.formats(timeout)?;
        let mime = crate::transfer::pick_text_type(&formats)?;
        String::from_utf8(
            c.read(mime, crate::CLIPBOARD_MAX, Duration::from_secs(1))
                .ok()??,
        )
        .ok()
    }

    fn clipboard_set(&self, text: &str) -> bool {
        let Some(c) = lock(&self.clipboard).clone() else {
            return false;
        };
        let mut o = Offers::new();
        for mime in crate::transfer::TEXT_TYPES
            .iter()
            .chain(["STRING", "TEXT"].iter())
        {
            o.insert(mime.to_string(), ClipData::Bytes(text.as_bytes().to_vec()));
        }
        c.put(&o)
    }

    fn can_place(&self) -> bool {
        self.sh.base.get_state().0 == "ok"
    }

    fn place_cursor(&self, x: i64, y: i64) -> bool {
        *lock(&self.sh.warp) = Some((x, y)); // done by the polling thread
        true
    }

    fn rich(&self) -> Option<Arc<dyn RichClipboard>> {
        lock(&self.clipboard)
            .clone()
            .map(|c| c as Arc<dyn RichClipboard>)
    }
}

// ============================================================================ clipboard
/// Bigger data is served in chunks (INCR).
const INCR_CHUNK: usize = 256 * 1024;

enum Cmd {
    Formats(mpsc::Sender<Option<Vec<String>>>),
    Read {
        mime: String,
        limit: usize,
        out: mpsc::Sender<Result<Option<Vec<u8>>, TooLarge>>,
        deadline: Instant,
    },
    Put(Offers, mpsc::Sender<bool>),
}

/// The CLIPBOARD selection, read and owned from a hidden window on its own connection and
/// thread. Requests from other threads go through a channel (and a pipe that wakes it).
pub struct X11Clipboard {
    cmds: Mutex<mpsc::Sender<Cmd>>,
    wake: OwnedFd,
    alive: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    prop: Atom,
    atom: Atom,
}

/// A read in progress.
struct Reading {
    target: Atom,
    limit: usize,
    deadline: Instant,
    data: Vec<u8>,
    incr: bool,
    reply: ReadReply,
}

enum ReadReply {
    Formats(mpsc::Sender<Option<Vec<String>>>),
    Data(mpsc::Sender<Result<Option<Vec<u8>>, TooLarge>>),
}

/// An INCR transfer we serve to a requestor.
struct Serving {
    window: Window,
    property: Atom,
    target: Atom,
    data: Vec<u8>,
    pos: usize,
    deadline: Instant,
}

struct Loop {
    conn: RustConnection,
    win: Window,
    atoms: Atoms,
    names: HashMap<Atom, String>,
    owned: Option<Offers>,
    reading: Option<Reading>,
    queue: VecDeque<Cmd>,
    serving: Vec<Serving>,
}

impl X11Clipboard {
    pub fn start() -> Result<Arc<Self>, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[screen].root;
        let win = conn.generate_id().map_err(|e| e.to_string())?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(|e| e.to_string())?;
        let intern = |name: &str| -> Result<Atom, String> {
            Ok(conn
                .intern_atom(false, name.as_bytes())
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?
                .atom)
        };
        let atoms = Atoms {
            clipboard: intern("CLIPBOARD")?,
            targets: intern("TARGETS")?,
            incr: intern("INCR")?,
            prop: intern(crate::local::X11_PROPERTY)?,
            atom: AtomEnum::ATOM.into(),
        };
        conn.flush().map_err(|e| e.to_string())?;
        let (rpipe, wpipe) = rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )
        .map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        let alive = Arc::new(AtomicBool::new(true));
        let a2 = alive.clone();
        let mut lp = Loop {
            conn,
            win,
            atoms,
            names: HashMap::new(),
            owned: None,
            reading: None,
            queue: VecDeque::new(),
            serving: Vec::new(),
        };
        let thread = std::thread::Builder::new()
            .name("flow-x11-clipboard".into())
            .spawn(move || {
                if let Err(e) = lp.run(rx, rpipe, &a2) {
                    log::info!("Flow: X11 clipboard stopped: {e}");
                }
                a2.store(false, Ordering::SeqCst);
            })
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            cmds: Mutex::new(tx),
            wake: wpipe,
            alive,
            thread: Mutex::new(Some(thread)),
        }))
    }

    fn send(&self, c: Cmd) -> bool {
        if lock(&self.cmds).send(c).is_err() {
            return false;
        }
        let _ = rustix::io::write(&self.wake, &[1]);
        true
    }
}

impl RichClipboard for X11Clipboard {
    fn kind(&self) -> &'static str {
        "x11"
    }

    fn available(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn formats(&self, timeout: Duration) -> Option<Vec<String>> {
        let (tx, rx) = mpsc::channel();
        if !self.send(Cmd::Formats(tx)) {
            return None;
        }
        rx.recv_timeout(timeout).ok().flatten()
    }

    fn read_to(
        &self,
        mime: &str,
        out: &mut dyn Write,
        limit: usize,
        timeout: Duration,
    ) -> Result<Option<u64>, TooLarge> {
        let (tx, rx) = mpsc::channel();
        let deadline = Instant::now() + timeout;
        if !self.send(Cmd::Read {
            mime: mime.to_string(),
            limit,
            out: tx,
            deadline,
        }) {
            return Ok(None);
        }
        match rx.recv_timeout(timeout + Duration::from_millis(200)) {
            Ok(Ok(Some(data))) => Ok(out.write_all(&data).ok().map(|()| data.len() as u64)),
            Ok(Err(TooLarge)) => Err(TooLarge),
            _ => Ok(None),
        }
    }

    fn put(&self, offers: &Offers) -> bool {
        let (tx, rx) = mpsc::channel();
        self.send(Cmd::Put(offers.clone(), tx))
            && rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
    }

    fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let _ = rustix::io::write(&self.wake, &[0]);
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
    }
}

type XResult<T> = Result<T, Box<dyn std::error::Error>>;

impl Loop {
    fn run(&mut self, rx: mpsc::Receiver<Cmd>, wake: OwnedFd, alive: &AtomicBool) -> XResult<()> {
        let mut drain = [0u8; 64];
        while alive.load(Ordering::SeqCst) {
            while let Some(ev) = self.conn.poll_for_event()? {
                self.event(ev)?;
            }
            while let Ok(c) = rx.try_recv() {
                self.queue.push_back(c);
            }
            self.next_command()?;
            self.timeouts()?;
            self.conn.flush()?;
            {
                use rustix::event::{PollFd, PollFlags, Timespec, poll};
                // SAFETY: the X connection's socket stays open while `self.conn` lives.
                let xfd = unsafe { BorrowedFd::borrow_raw(self.conn.stream().as_raw_fd()) };
                let mut fds = [
                    PollFd::new(&xfd, PollFlags::IN),
                    PollFd::new(&wake, PollFlags::IN),
                ];
                let ts = Timespec {
                    tv_sec: 0,
                    tv_nsec: 200_000_000,
                };
                poll(&mut fds, Some(&ts))?;
            }
            while rustix::io::read(&wake, &mut drain).is_ok_and(|n| n > 0) {}
        }
        Ok(())
    }

    fn atom(&mut self, name: &str) -> XResult<Atom> {
        if let Some((a, _)) = self.names.iter().find(|(_, n)| n.as_str() == name) {
            return Ok(*a);
        }
        let a = self.conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
        self.names.insert(a, name.to_string());
        Ok(a)
    }

    fn name(&mut self, atom: Atom) -> XResult<String> {
        if let Some(n) = self.names.get(&atom) {
            return Ok(n.clone());
        }
        let n = String::from_utf8_lossy(&self.conn.get_atom_name(atom)?.reply()?.name).into_owned();
        self.names.insert(atom, n.clone());
        Ok(n)
    }

    fn next_command(&mut self) -> XResult<()> {
        while self.reading.is_none() {
            let Some(c) = self.queue.pop_front() else {
                return Ok(());
            };
            match c {
                Cmd::Put(offers, tx) => {
                    self.conn.set_selection_owner(
                        self.win,
                        self.atoms.clipboard,
                        x11rb::CURRENT_TIME,
                    )?;
                    let owner = self
                        .conn
                        .get_selection_owner(self.atoms.clipboard)?
                        .reply()?
                        .owner;
                    let ok = owner == self.win;
                    if ok {
                        self.owned = Some(offers);
                    }
                    let _ = tx.send(ok);
                }
                Cmd::Formats(tx) => self.start_read(
                    self.atoms.targets,
                    1 << 16,
                    Instant::now() + Duration::from_secs(1),
                    ReadReply::Formats(tx),
                )?,
                Cmd::Read {
                    mime,
                    limit,
                    out,
                    deadline,
                } => {
                    let target = self.atom(&mime)?;
                    self.start_read(target, limit, deadline, ReadReply::Data(out))?;
                }
            }
        }
        Ok(())
    }

    fn start_read(
        &mut self,
        target: Atom,
        limit: usize,
        deadline: Instant,
        reply: ReadReply,
    ) -> XResult<()> {
        self.conn.delete_property(self.win, self.atoms.prop)?;
        self.conn.convert_selection(
            self.win,
            self.atoms.clipboard,
            target,
            self.atoms.prop,
            x11rb::CURRENT_TIME,
        )?;
        self.reading = Some(Reading {
            target,
            limit,
            deadline,
            data: Vec::new(),
            incr: false,
            reply,
        });
        Ok(())
    }

    fn finish_read(&mut self, data: Option<Vec<u8>>, too_large: bool) -> XResult<()> {
        let Some(r) = self.reading.take() else {
            return Ok(());
        };
        match r.reply {
            ReadReply::Data(tx) => {
                let _ = tx.send(if too_large { Err(TooLarge) } else { Ok(data) });
            }
            ReadReply::Formats(tx) => {
                let list = match data {
                    Some(d) => {
                        let mut out = Vec::new();
                        for c in d.chunks_exact(4).take(256) {
                            let a = u32::from_ne_bytes([c[0], c[1], c[2], c[3]]);
                            out.push(self.name(a)?);
                        }
                        Some(out)
                    }
                    None => Some(Vec::new()), // nobody owns the clipboard
                };
                let _ = tx.send(list);
            }
        }
        Ok(())
    }

    /// Reads (and deletes) our property: (type, bytes).
    fn take_property(&mut self) -> XResult<(Atom, Vec<u8>)> {
        let r = self
            .conn
            .get_property(
                true,
                self.win,
                self.atoms.prop,
                AtomEnum::ANY,
                0,
                u32::MAX / 4,
            )?
            .reply()?;
        Ok((r.type_, r.value))
    }

    fn timeouts(&mut self) -> XResult<()> {
        if self
            .reading
            .as_ref()
            .is_some_and(|r| Instant::now() > r.deadline)
        {
            self.finish_read(None, false)?;
        }
        let now = Instant::now();
        self.serving.retain(|s| now < s.deadline);
        Ok(())
    }

    fn event(&mut self, ev: Event) -> XResult<()> {
        match ev {
            Event::SelectionNotify(e) if e.requestor == self.win => {
                let Some(target) = self.reading.as_ref().map(|r| r.target) else {
                    return Ok(());
                };
                if e.property == x11rb::NONE || e.target != target {
                    return self.finish_read(None, false);
                }
                let (type_, value) = self.take_property()?;
                if type_ == self.atoms.incr {
                    if let Some(r) = self.reading.as_mut() {
                        r.incr = true; // chunks follow as property changes
                    }
                    return Ok(());
                }
                let limit = self.reading.as_ref().map(|r| r.limit).unwrap_or(0);
                if value.len() > limit {
                    return self.finish_read(None, true);
                }
                self.finish_read(Some(value), false)
            }
            Event::PropertyNotify(e)
                if e.window == self.win
                    && e.atom == self.atoms.prop
                    && e.state == Property::NEW_VALUE =>
            {
                if !self.reading.as_ref().is_some_and(|r| r.incr) {
                    return Ok(());
                }
                let (_t, chunk) = self.take_property()?;
                let Some(r) = self.reading.as_mut() else {
                    return Ok(());
                };
                if chunk.is_empty() {
                    let data = std::mem::take(&mut r.data);
                    return self.finish_read(Some(data), false);
                }
                r.data.extend_from_slice(&chunk);
                if r.data.len() > r.limit {
                    return self.finish_read(None, true);
                }
                Ok(())
            }
            Event::PropertyNotify(e) if e.state == Property::DELETE => {
                // a requestor took a chunk of an INCR transfer: the next one
                let Some(i) = self
                    .serving
                    .iter()
                    .position(|s| s.window == e.window && s.property == e.atom)
                else {
                    return Ok(());
                };
                let s = &mut self.serving[i];
                let end = (s.pos + INCR_CHUNK).min(s.data.len());
                let chunk = s.data[s.pos..end].to_vec();
                s.pos = end;
                let (w, p, target) = (s.window, s.property, s.target);
                self.conn.change_property(
                    PropMode::REPLACE,
                    w,
                    p,
                    target,
                    8,
                    chunk.len() as u32,
                    &chunk,
                )?;
                if chunk.is_empty() {
                    self.serving.remove(i);
                }
                Ok(())
            }
            Event::SelectionClear(e) if e.selection == self.atoms.clipboard => {
                self.owned = None;
                Ok(())
            }
            Event::SelectionRequest(e) => {
                let mut notify = SelectionNotifyEvent {
                    response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: e.time,
                    requestor: e.requestor,
                    selection: e.selection,
                    target: e.target,
                    property: x11rb::NONE,
                };
                let property = if e.property == x11rb::NONE {
                    e.target
                } else {
                    e.property
                };
                if e.selection == self.atoms.clipboard {
                    if let Some(offers) = self.owned.clone() {
                        let target = self.name(e.target)?;
                        if e.target == self.atoms.targets {
                            let mut atoms = vec![self.atoms.targets];
                            for mime in offers.keys() {
                                atoms.push(self.atom(mime)?);
                            }
                            let bytes: Vec<u8> =
                                atoms.iter().flat_map(|a| a.to_ne_bytes()).collect();
                            self.conn.change_property(
                                PropMode::REPLACE,
                                e.requestor,
                                property,
                                self.atoms.atom,
                                32,
                                atoms.len() as u32,
                                &bytes,
                            )?;
                            notify.property = property;
                        } else if let Some(data) = offers.get(&target) {
                            let mut bytes = Vec::new();
                            if copy_out(data, &mut |c| {
                                bytes.extend_from_slice(c);
                                Ok(())
                            })
                            .is_ok()
                            {
                                if bytes.len() > INCR_CHUNK {
                                    self.conn.change_window_attributes(
                                        e.requestor,
                                        &ChangeWindowAttributesAux::new()
                                            .event_mask(EventMask::PROPERTY_CHANGE),
                                    )?;
                                    let size = (bytes.len() as u32).to_ne_bytes();
                                    self.conn.change_property(
                                        PropMode::REPLACE,
                                        e.requestor,
                                        property,
                                        self.atoms.incr,
                                        32,
                                        1,
                                        &size,
                                    )?;
                                    self.serving.push(Serving {
                                        window: e.requestor,
                                        property,
                                        target: e.target,
                                        data: bytes,
                                        pos: 0,
                                        deadline: Instant::now() + Duration::from_secs(30),
                                    });
                                } else {
                                    self.conn.change_property(
                                        PropMode::REPLACE,
                                        e.requestor,
                                        property,
                                        e.target,
                                        8,
                                        bytes.len() as u32,
                                        &bytes,
                                    )?;
                                }
                                notify.property = property;
                            }
                        }
                    }
                }
                self.conn
                    .send_event(false, e.requestor, EventMask::NO_EVENT, notify)?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct FakeX {
        pos: Arc<Mutex<(i64, i64, u16)>>,
        warps: Arc<Mutex<Vec<(i64, i64)>>>,
    }

    impl PointerSource for FakeX {
        fn pointer(&mut self) -> Option<(i64, i64, u16)> {
            Some(*self.pos.lock().unwrap())
        }
        fn screens(&mut self) -> Vec<Rect> {
            vec![[0, 0, 1920, 1080]]
        }
        fn warp(&mut self, x: i64, y: i64) {
            self.warps.lock().unwrap().push((x, y));
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<VecDeque<String>>);

    impl Sink for Recorder {
        fn edge(&self, side: &'static str, x: f64, y: f64, _r: Rect) {
            self.0
                .lock()
                .unwrap()
                .push_back(format!("edge {side} {x} {y}"));
        }
        fn leave(&self, side: &'static str) {
            self.0.lock().unwrap().push_back(format!("leave {side}"));
        }
        fn ctrl(&self, held: bool) {
            self.0.lock().unwrap().push_back(format!("ctrl {held}"));
        }
        fn screens(&self, _r: Vec<Rect>) {}
        fn state(&self, state: &str, _m: Option<Text>) {
            self.0.lock().unwrap().push_back(format!("state {state}"));
        }
    }

    fn wait(mut f: impl FnMut() -> bool) -> bool {
        (0..100).any(|_| {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
            false
        })
    }

    #[test]
    fn pointer_edges_ctrl_buttons_and_warp() {
        let pos = Arc::new(Mutex::new((900, 500, 0u16)));
        let warps = Arc::new(Mutex::new(Vec::new()));
        let (p2, w2) = (pos.clone(), warps.clone());
        let b = X11Backend::with_source(
            Box::new(move || {
                Ok(Box::new(FakeX {
                    pos: p2.clone(),
                    warps: w2.clone(),
                }) as Box<dyn PointerSource>)
            }),
            false,
        );
        let sink = Arc::new(Recorder::default());
        b.start(sink.clone());
        let has = |s: &str| sink.0.lock().unwrap().iter().any(|e| e == s);
        assert!(wait(|| has("state ok")));
        *pos.lock().unwrap() = (1919, 500, CONTROL_MASK); // right edge with Ctrl
        assert!(wait(|| has("edge right 1919 500")));
        assert_eq!(b.ctrl_held(), Some(true));
        assert_eq!(b.buttons_held(), Some(false));
        *pos.lock().unwrap() = (900, 500, 0);
        assert!(wait(|| has("leave right")));
        *pos.lock().unwrap() = (0, 500, 0x100); // left button held: dragging
        std::thread::sleep(Duration::from_millis(150));
        assert!(!has("edge left 0 500"));
        assert_eq!(b.buttons_held(), Some(true));
        *pos.lock().unwrap() = (0, 500, 0); // released at the edge: not a push
        std::thread::sleep(Duration::from_millis(150));
        assert!(!has("edge left 0 500"));
        *pos.lock().unwrap() = (0, 530, 0); // pushing along it
        assert!(wait(|| has("edge left 0 530")));
        assert!(b.place_cursor(10, 20));
        assert!(wait(|| *warps.lock().unwrap() == vec![(10, 20)]));
        b.stop();
    }
}
