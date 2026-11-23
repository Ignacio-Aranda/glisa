//! Where is the pointer, and how do we put it somewhere?
//!
//! ```text
//!     linux::kwin::KWinBackend .. KDE Plasma on Wayland: KWin script over D-Bus (edges),
//!                                 org_kde_kwin_keystate (Ctrl), evdev (mouse buttons),
//!                                 uinput absolute pointer (placement), klipper / Wayland
//!                                 data-control (clipboard)
//!     linux::x11::X11Backend .... any X11 session: pointer polling, XWarpPointer, selections
//!     windows::WindowsBackend ... GetCursorPos polling, SetCursorPos, the Windows clipboard
//!     macos::MacBackend ......... CGEvent location polling, CGWarpMouseCursorPosition,
//!                                 NSPasteboard
//!     FakeBackend ............... tests and demo mode: driven by hand, records what it is asked
//! ```
//!
//! Every backend reports to a [`Sink`]: edge(side, x, y, rect), leave(side), screens(rects),
//! ctrl(held), state(state, message). No backend lets the pointer cross while a mouse
//! button is held (dragging a window or a file to the edge is not a switch): the polling
//! backends never report an edge while a button is held, and every backend answers
//! [`EdgeBackend::buttons_held`], which the engine checks before switching.
//!
//! The exception is a file drag (KDE Wayland only): while a button is held at an edge that
//! leads to a peer, the engine asks for the edge catcher ([`EdgeBackend::drag_catcher`]);
//! files dragged onto it are reported with [`Sink::file_drag`], and only then may the
//! pointer cross with the button held.
//!
//! Files dragged anywhere (KDE Wayland only): while the engine wants it
//! ([`EdgeBackend::drag_watch`]), the backend reports a file drag starting and the button
//! holding it let go ([`Sink::files_dragging`]), and the engine makes the edges that lead to
//! a peer glow meanwhile ([`EdgeBackend::drag_glow`]).
//!
//! Files dragged here from another computer (KDE Wayland only): the engine tells the
//! backend what to expect ([`EdgeBackend::drag_pickup`]); the press of the arriving mouse,
//! whose button is still held, picks the drag up again, and the backend reports how it went
//! ([`Sink::pickup`]).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glisa_i18n::n_;

use crate::clipboard::{FakeClipboard, RichClipboard};
use crate::geometry::{
    DEFAULT_SCREEN, EdgeEvent, EdgeTracker, GlowEdge, Rect, bounding, clean_rects, now_s,
    point_on_edge,
};
use crate::text::Text;
use crate::util::lock;

pub use crate::text::UNAVAILABLE;

/// What backends report to the engine.
pub trait Sink: Send + Sync {
    fn edge(&self, side: &'static str, x: f64, y: f64, rect: Rect);
    fn leave(&self, side: &'static str);
    fn ctrl(&self, held: bool);
    fn screens(&self, rects: Vec<Rect>);
    fn state(&self, state: &str, message: Option<Text>);
    /// Files were dragged onto the edge catcher on `side` (local paths, already checked).
    fn file_drag(&self, _side: &'static str, _paths: Vec<PathBuf>) {}
    /// Would a file drag make some edge glow now? The drag watch asks before it looks.
    fn drag_glow_wanted(&self) -> bool {
        false
    }
    /// A file drag started somewhere on the desktop (true), or the button holding it was
    /// let go (false: dropped or cancelled).
    fn files_dragging(&self, _on: bool) {}
    /// How the drag picked up here went.
    fn pickup(&self, _news: PickupNews) {}
}

/// A step of a drag picked up on this computer (see [`EdgeBackend::drag_pickup`]).
#[derive(Debug, Clone, PartialEq)]
pub enum PickupStep {
    /// Files dragged on another computer are coming with the mouse: catch its press on the
    /// output `output` (where the pointer will enter); `count` items.
    Expect { output: Rect, count: usize },
    /// Our mouse is here: a press of its left button on the pickup surface starts the drag.
    Arrived,
    /// Every file is here (top-level items in the staging folder): what a drop gets.
    Ready(Vec<PathBuf>),
    /// Over: the pickup surface goes (a drag already going on is cancelled).
    Cancel,
}

/// How a drag picked up here went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickupNews {
    /// The press came: the drag goes on here.
    Started,
    /// The drag ended: `taken`, a drop target got the files (else it was cancelled, or the
    /// files weren't all here in time).
    Ended { taken: bool },
}

/// Forwards tracker events to a sink.
pub fn report(sink: &dyn Sink, events: Vec<EdgeEvent>) {
    for ev in events {
        match ev {
            EdgeEvent::Edge { side, x, y, rect } => sink.edge(side, x, y, rect),
            EdgeEvent::Leave { side } => sink.leave(side),
        }
    }
}

pub trait EdgeBackend: Send + Sync {
    /// "kwin", "x11", "windows", "macos", "fake", "none".
    fn kind(&self) -> &'static str;
    fn start(&self, sink: Arc<dyn Sink>);
    fn stop(&self);
    /// (off | starting | ok | error | unavailable, message).
    fn state(&self) -> (String, Option<Text>);
    fn screens(&self) -> Vec<Rect>;
    fn ctrl_held(&self) -> Option<bool> {
        None
    }
    fn ctrl_available(&self) -> bool {
        false
    }
    /// Is a mouse button held right now? None: this desktop can't tell.
    fn buttons_held(&self) -> Option<bool> {
        None
    }
    /// Where clipboard text is read and written ("none": nowhere, the app's fallback).
    fn clipboard_kind(&self) -> &'static str {
        "none"
    }
    fn clipboard_get(&self, _timeout: Duration) -> Option<String> {
        None
    }
    fn clipboard_set(&self, _text: &str) -> bool {
        false
    }
    fn can_place(&self) -> bool {
        false
    }
    fn place_cursor(&self, _x: i64, _y: i64) -> bool {
        false
    }
    /// Every MIME type of the clipboard (copied files, images), when the desktop allows it.
    fn rich(&self) -> Option<Arc<dyn RichClipboard>> {
        None
    }
    /// Can files dragged to the edge be caught here (KDE Plasma on Wayland)?
    fn drag_catcher_available(&self) -> bool {
        false
    }
    /// Shows the edge catcher on `side` of the output `rect` (Some), or takes it away
    /// (None; while a drag is on it, it stays until that drag is over). Files dragged onto
    /// it are reported with [`Sink::file_drag`].
    fn drag_catcher(&self, _at: Option<(&'static str, Rect)>) {}
    /// The crossing for the drag reported last goes ahead (true: take its drop, as a copy)
    /// or not (false: refuse it). Returns whether that drag is still on the catcher.
    fn drag_accept(&self, _accept: bool) -> bool {
        false
    }
    /// Watch for files dragged anywhere on the desktop (on) or stop (off); reported with
    /// [`Sink::files_dragging`].
    fn drag_watch(&self, _on: bool) {}
    /// Shows the drag glow along `edges` (empty: takes it away). Visual only.
    fn drag_glow(&self, _edges: &[GlowEdge]) {}
    /// Can a drag from another computer go on here (KDE Plasma on Wayland)?
    fn drag_pickup_available(&self) -> bool {
        false
    }
    /// The next step of a drag picked up here; news come with [`Sink::pickup`].
    fn drag_pickup(&self, _step: PickupStep) {}
    fn as_fake(&self) -> Option<&FakeBackend> {
        None
    }
}

/// State and sink every backend keeps.
#[derive(Default)]
pub struct Base {
    state: Mutex<(String, Option<Text>)>,
    sink: Mutex<Option<Arc<dyn Sink>>>,
}

impl Base {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(("off".into(), None)),
            sink: Mutex::new(None),
        }
    }

    pub fn set_sink(&self, sink: Option<Arc<dyn Sink>>) {
        *lock(&self.sink) = sink;
    }

    pub fn sink(&self) -> Option<Arc<dyn Sink>> {
        lock(&self.sink).clone()
    }

    pub fn set_state(&self, state: &str, message: Option<Text>) {
        *lock(&self.state) = (state.to_string(), message.clone());
        if let Some(s) = self.sink() {
            s.state(state, message);
        }
    }

    pub fn get_state(&self) -> (String, Option<Text>) {
        lock(&self.state).clone()
    }
}

/// Which desktop backend this session can use: `(Some(kind), None)` or `(None, why)`.
/// `env` is the environment (tests pass their own).
pub fn detect(env: &dyn Fn(&str) -> Option<String>) -> (Option<&'static str>, Option<Text>) {
    if cfg!(windows) {
        return (Some("windows"), None);
    }
    if cfg!(target_os = "macos") {
        return (Some("macos"), None);
    }
    let get = |k: &str| env(k).filter(|v| !v.is_empty());
    let mut session = get("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase();
    if session.is_empty() {
        session = if get("WAYLAND_DISPLAY").is_some() {
            "wayland".into()
        } else if get("DISPLAY").is_some() {
            "x11".into()
        } else {
            String::new()
        };
    }
    let desktop = get("XDG_CURRENT_DESKTOP")
        .or_else(|| get("DESKTOP_SESSION"))
        .or_else(|| get("KDE_FULL_SESSION").map(|_| "KDE".to_string()))
        .unwrap_or_default()
        .to_uppercase();
    if session == "wayland" {
        if desktop.split(':').any(|d| d == "KDE")
            || desktop.contains("KDE")
            || desktop.contains("PLASMA")
        {
            return (Some("kwin"), None);
        }
        let name = get("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            return (
                None,
                Some(Text::new(n_!(
                    "Flow is not available on this desktop: on Wayland, {app} can only track the pointer on KDE Plasma"
                ))
                .arg("app", glisa_common::APP_NAME)),
            );
        }
        return (
            None,
            Some(
                Text::new(n_!(
                    "Flow is not available on this desktop: on Wayland, {app} can only track the pointer on KDE Plasma ({desktop} doesn't allow it)"
                ))
                .arg("app", glisa_common::APP_NAME)
                .arg("desktop", name),
            ),
        );
    }
    if session == "x11" {
        if get("DISPLAY").is_none() {
            return (
                None,
                Some(Text::new(n_!(
                    "Flow is not available on this desktop: the X11 display can't be found"
                ))),
            );
        }
        return (Some("x11"), None);
    }
    (
        None,
        Some(Text::new(n_!(
            "Flow is not available on this desktop: no graphical session found"
        ))),
    )
}

/// [`detect`] on this process's environment.
pub fn detect_here() -> (Option<&'static str>, Option<Text>) {
    detect(&|k| std::env::var(k).ok())
}

/// The backend for this desktop.
pub fn make_backend() -> Arc<dyn EdgeBackend> {
    let (kind, reason) = detect_here();
    match kind {
        #[cfg(target_os = "linux")]
        Some("kwin") => Arc::new(crate::linux::kwin::KWinHandle::new()),
        #[cfg(target_os = "linux")]
        Some("x11") => Arc::new(crate::linux::x11::X11Backend::new()),
        #[cfg(windows)]
        Some("windows") => Arc::new(crate::windows::WindowsBackend::new()),
        #[cfg(target_os = "macos")]
        Some("macos") => Arc::new(crate::macos::MacBackend::new()),
        _ => Arc::new(UnavailableBackend::new(reason.unwrap_or_else(|| {
            Text::new(n_!(
                "Flow is not available on this desktop: no graphical session found"
            ))
        }))),
    }
}

pub struct UnavailableBackend {
    base: Base,
    reason: Text,
}

impl UnavailableBackend {
    pub fn new(reason: Text) -> Self {
        Self {
            base: Base::new(),
            reason,
        }
    }
}

impl EdgeBackend for UnavailableBackend {
    fn kind(&self) -> &'static str {
        "none"
    }
    fn start(&self, sink: Arc<dyn Sink>) {
        self.base.set_sink(Some(sink));
        self.base
            .set_state("unavailable", Some(self.reason.clone()));
    }
    fn stop(&self) {
        self.base.set_state("off", None);
        self.base.set_sink(None);
    }
    fn state(&self) -> (String, Option<Text>) {
        self.base.get_state()
    }
    fn screens(&self) -> Vec<Rect> {
        Vec::new()
    }
}

// ================================================================ absolute pointer
pub const EV_SYN: u16 = 0;
pub const EV_ABS: u16 = 3;
pub const SYN_REPORT: u16 = 0;
pub const ABS_X: u16 = 0;
pub const ABS_Y: u16 = 1;
pub const ABS_MAX: i32 = 65535;

/// Axis value that lands on pixel `coord` when the compositor maps the full range onto
/// `extent` pixels starting at `origin` (libinput: `value × size / (max − min + 1)`).
pub fn abs_value_for(coord: f64, extent: f64, origin: f64) -> i32 {
    if extent <= 0.0 {
        return 0;
    }
    let v = ((coord - origin + 0.5) * (ABS_MAX as f64 + 1.0) / extent).trunc();
    v.clamp(0.0, ABS_MAX as f64) as i32
}

/// Events an absolute pointer would emit for a move (dry run: recorded, never sent).
#[derive(Default)]
pub struct DryRunPointer {
    pub events: Mutex<Vec<(u16, u16, i32)>>,
}

impl DryRunPointer {
    pub fn move_to(&self, ax: i32, ay: i32) {
        lock(&self.events).extend([
            (EV_ABS, ABS_X, ax),
            (EV_ABS, ABS_Y, ay),
            (EV_SYN, SYN_REPORT, 0),
        ]);
    }
}

// ======================================================================= fake
/// Pointer driven by tests/demo. Placement goes through a dry-run absolute pointer so the
/// absolute values are checked too.
pub struct FakeBackend {
    base: Base,
    screens: Vec<Rect>,
    pub rich_clip: Option<Arc<FakeClipboard>>,
    tracker: Mutex<EdgeTracker>,
    pub ctrl_supported: AtomicBool,
    ctrl: AtomicBool,
    /// Clipboard text (None: no text clipboard).
    pub clip: Mutex<Option<String>>,
    pub pointer: DryRunPointer,
    pub placements: Mutex<Vec<(i64, i64)>>,
    pos: Mutex<(f64, f64)>,
    /// What `buttons_held` answers (None: "can't tell").
    pub buttons: Mutex<Option<bool>>,
    /// Whether the edge catcher exists on this fake desktop.
    pub catcher_supported: AtomicBool,
    /// Where the catcher is now, and every request (Some: shown, None: removed).
    pub catcher: Mutex<Option<(&'static str, Rect)>>,
    pub catcher_log: Mutex<Vec<Option<(&'static str, Rect)>>>,
    /// The drag is on the catcher (answered by `drag_accept`), and what it was told.
    pub drag_on_catcher: AtomicBool,
    pub accepted: Mutex<Vec<bool>>,
    /// Tests: the button is let go (the drop refused) before the crossing accepts it.
    pub drop_before_accept: AtomicBool,
    /// The drag watch is on.
    pub watching: AtomicBool,
    /// The drag glow on screen now, and every request.
    pub glow: Mutex<Vec<GlowEdge>>,
    pub glow_log: Mutex<Vec<Vec<GlowEdge>>>,
    /// Drags from another computer can be picked up here (off: they come as copied files).
    pub pickup_supported: AtomicBool,
    /// Every pickup step asked for.
    pub pickup_log: Mutex<Vec<PickupStep>>,
}

impl FakeBackend {
    pub fn new(screens: &[Rect]) -> Self {
        Self::with(screens, true, true, true)
    }

    pub fn with(screens: &[Rect], ctrl_available: bool, clipboard: bool, rich: bool) -> Self {
        let mut screens = clean_rects(screens);
        if screens.is_empty() {
            screens.push(DEFAULT_SCREEN);
        }
        let [bx, by, bw, bh] = bounding(&screens);
        Self {
            base: Base::new(),
            pos: Mutex::new(((bx + bw / 2) as f64, (by + bh / 2) as f64)),
            screens,
            rich_clip: rich.then(|| Arc::new(FakeClipboard::default())),
            tracker: Mutex::new(EdgeTracker::default()),
            ctrl_supported: AtomicBool::new(ctrl_available),
            ctrl: AtomicBool::new(false),
            clip: Mutex::new(clipboard.then(String::new)),
            pointer: DryRunPointer::default(),
            placements: Mutex::new(Vec::new()),
            buttons: Mutex::new(Some(false)),
            catcher_supported: AtomicBool::new(true),
            catcher: Mutex::new(None),
            catcher_log: Mutex::new(Vec::new()),
            drag_on_catcher: AtomicBool::new(false),
            accepted: Mutex::new(Vec::new()),
            drop_before_accept: AtomicBool::new(false),
            watching: AtomicBool::new(false),
            glow: Mutex::new(Vec::new()),
            glow_log: Mutex::new(Vec::new()),
            pickup_supported: AtomicBool::new(false),
            pickup_log: Mutex::new(Vec::new()),
        }
    }

    pub fn move_to(&self, x: f64, y: f64) {
        let events = {
            let mut tr = lock(&self.tracker);
            *lock(&self.pos) = (x, y);
            tr.feed(x, y, &self.screens, now_s(), false)
        };
        if let Some(sink) = self.base.sink() {
            report(sink.as_ref(), events);
        }
    }

    /// Push the pointer against `side` at relative position `t`.
    pub fn hit(&self, side: &str, t: f64) {
        let (x, y) = point_on_edge(&self.screens, side, t);
        let (x, y) = (x as f64, y as f64);
        let same = lock(&self.tracker).side == crate::side_of(side) && *lock(&self.pos) == (x, y);
        if same {
            let (nx, ny) = match side {
                "left" => (x + 1.0, y),
                "right" => (x - 1.0, y),
                "top" => (x, y + 1.0),
                _ => (x, y - 1.0),
            };
            self.move_to(nx, ny); // nudge so it counts as a new event
        }
        self.move_to(x, y);
    }

    /// Back to the middle of the desktop (leaves any edge).
    pub fn away(&self) {
        let [bx, by, bw, bh] = bounding(&self.screens);
        self.move_to((bx + bw / 2) as f64, (by + bh / 2) as f64);
    }

    pub fn set_ctrl(&self, held: bool) {
        self.ctrl.store(held, Ordering::SeqCst);
        if let Some(s) = self.base.sink() {
            s.ctrl(held);
        }
    }

    pub fn clip(&self) -> Option<String> {
        lock(&self.clip).clone()
    }

    pub fn set_clip(&self, text: &str) {
        *lock(&self.clip) = Some(text.to_string());
    }

    pub fn placements(&self) -> Vec<(i64, i64)> {
        lock(&self.placements).clone()
    }

    /// The user drags files onto the catcher (as a file manager offers them). False when
    /// there is no catcher to drag onto.
    pub fn drag_files(&self, paths: &[PathBuf]) -> bool {
        let Some((side, _)) = *lock(&self.catcher) else {
            return false;
        };
        let uris: String = paths
            .iter()
            .map(|p| crate::transfer::uri_of(p) + "\r\n")
            .collect();
        let paths = crate::drag::parse_drag_uris(uris.as_bytes());
        if paths.is_empty() {
            return false;
        }
        self.drag_on_catcher.store(
            !self.drop_before_accept.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        if let Some(sink) = self.base.sink() {
            sink.file_drag(side, paths);
        }
        true
    }

    pub fn catcher(&self) -> Option<(&'static str, Rect)> {
        *lock(&self.catcher)
    }

    pub fn catcher_log(&self) -> Vec<Option<(&'static str, Rect)>> {
        lock(&self.catcher_log).clone()
    }

    pub fn accepted(&self) -> Vec<bool> {
        lock(&self.accepted).clone()
    }

    /// The user picks up files somewhere on the desktop: found as the drag watch would (it
    /// is on, and the engine wants to know). False when nothing looked for it.
    pub fn start_file_drag(&self) -> bool {
        if !self.watching.load(Ordering::SeqCst) {
            return false;
        }
        let Some(sink) = self.base.sink() else {
            return false;
        };
        if !sink.drag_glow_wanted() {
            return false;
        }
        sink.files_dragging(true);
        true
    }

    /// The button holding the drag is let go (dropped or cancelled).
    pub fn end_file_drag(&self) {
        if let Some(sink) = self.base.sink() {
            sink.files_dragging(false);
        }
    }

    pub fn glow(&self) -> Vec<GlowEdge> {
        lock(&self.glow).clone()
    }

    pub fn glow_log(&self) -> Vec<Vec<GlowEdge>> {
        lock(&self.glow_log).clone()
    }

    pub fn pickup_log(&self) -> Vec<PickupStep> {
        lock(&self.pickup_log).clone()
    }

    /// The arriving mouse's press lands on the pickup surface: the drag goes on here.
    /// False when no pickup is armed (not expected, or the mouse isn't here yet).
    pub fn pickup_press(&self) -> bool {
        let log = self.pickup_log();
        let armed = log
            .iter()
            .rposition(|s| matches!(s, PickupStep::Expect { .. }));
        let ok = armed.is_some_and(|i| {
            log[i..].contains(&PickupStep::Arrived) && !log[i..].contains(&PickupStep::Cancel)
        });
        if ok {
            if let Some(s) = self.base.sink() {
                s.pickup(PickupNews::Started);
            }
        }
        ok
    }

    /// The user lets go over a folder (`accept`) or somewhere that refuses it. The files the
    /// drop got (they must be ready by then); empty when it got nothing.
    pub fn pickup_drop(&self, accept: bool) -> Vec<PathBuf> {
        let ready = self.pickup_log().into_iter().rev().find_map(|s| match s {
            PickupStep::Ready(p) => Some(p),
            _ => None,
        });
        let got = match ready {
            Some(p) if accept => p,
            _ => Vec::new(),
        };
        if let Some(s) = self.base.sink() {
            s.pickup(PickupNews::Ended {
                taken: !got.is_empty(),
            });
        }
        got
    }

    /// The desktop reports these screens (the fake pointer keeps its own).
    pub fn report_screens(&self, rects: Vec<Rect>) {
        if let Some(sink) = self.base.sink() {
            sink.screens(rects);
        }
    }

    pub fn rich_data(&self) -> BTreeMap<String, Vec<u8>> {
        self.rich_clip
            .as_ref()
            .map(|c| c.snapshot())
            .unwrap_or_default()
    }
}

impl EdgeBackend for FakeBackend {
    fn kind(&self) -> &'static str {
        "fake"
    }
    fn start(&self, sink: Arc<dyn Sink>) {
        self.base.set_sink(Some(sink.clone()));
        self.base.set_state("ok", None);
        sink.screens(self.screens.clone());
    }
    fn stop(&self) {
        *lock(&self.base.state) = ("off".into(), None);
        self.base.set_sink(None);
        self.watching.store(false, Ordering::SeqCst);
        lock(&self.glow).clear();
    }
    fn state(&self) -> (String, Option<Text>) {
        self.base.get_state()
    }
    fn screens(&self) -> Vec<Rect> {
        self.screens.clone()
    }
    fn ctrl_held(&self) -> Option<bool> {
        self.ctrl_available()
            .then(|| self.ctrl.load(Ordering::SeqCst))
    }
    fn ctrl_available(&self) -> bool {
        self.ctrl_supported.load(Ordering::SeqCst)
    }
    fn buttons_held(&self) -> Option<bool> {
        *lock(&self.buttons)
    }
    fn clipboard_kind(&self) -> &'static str {
        if lock(&self.clip).is_some() {
            "fake"
        } else {
            "none"
        }
    }
    fn clipboard_get(&self, _timeout: Duration) -> Option<String> {
        lock(&self.clip).clone()
    }
    fn clipboard_set(&self, text: &str) -> bool {
        let mut c = lock(&self.clip);
        if c.is_none() {
            return false;
        }
        *c = Some(text.to_string());
        true
    }
    fn can_place(&self) -> bool {
        true
    }
    fn place_cursor(&self, x: i64, y: i64) -> bool {
        let [bx, by, bw, bh] = bounding(&self.screens);
        self.pointer.move_to(
            abs_value_for(x as f64, (bx + bw) as f64, 0.0),
            abs_value_for(y as f64, (by + bh) as f64, 0.0),
        );
        lock(&self.placements).push((x, y));
        self.move_to(x as f64, y as f64); // what the compositor would report back
        true
    }
    fn rich(&self) -> Option<Arc<dyn RichClipboard>> {
        self.rich_clip.clone().map(|c| c as Arc<dyn RichClipboard>)
    }
    fn drag_catcher_available(&self) -> bool {
        self.catcher_supported.load(Ordering::SeqCst)
    }
    fn drag_catcher(&self, at: Option<(&'static str, Rect)>) {
        if at.is_none() {
            self.drag_on_catcher.store(false, Ordering::SeqCst);
        }
        *lock(&self.catcher) = at;
        lock(&self.catcher_log).push(at);
    }
    fn drag_accept(&self, accept: bool) -> bool {
        lock(&self.accepted).push(accept);
        self.drag_on_catcher.load(Ordering::SeqCst)
    }
    fn drag_watch(&self, on: bool) {
        self.watching.store(on, Ordering::SeqCst);
    }
    fn drag_glow(&self, edges: &[GlowEdge]) {
        *lock(&self.glow) = edges.to_vec();
        lock(&self.glow_log).push(edges.to_vec());
    }
    fn drag_pickup_available(&self) -> bool {
        self.pickup_supported.load(Ordering::SeqCst)
    }
    fn drag_pickup(&self, step: PickupStep) {
        lock(&self.pickup_log).push(step);
    }
    fn as_fake(&self) -> Option<&FakeBackend> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_values() {
        assert_eq!(abs_value_for(0.0, 1920.0, 0.0), 17);
        assert_eq!(abs_value_for(1919.0, 1920.0, 0.0), 65518);
        assert_eq!(abs_value_for(5000.0, 1920.0, 0.0), 65535);
        assert_eq!(abs_value_for(-5.0, 1920.0, 0.0), 0);
        for x in [0, 1, 959, 1919] {
            let v = abs_value_for(x as f64, 1920.0, 0.0);
            assert_eq!((v as i64 * 1920) / 65536, x); // libinput lands on x
        }
        let p = DryRunPointer::default();
        p.move_to(100, 200);
        assert_eq!(
            *lock(&p.events),
            vec![(EV_ABS, ABS_X, 100), (EV_ABS, ABS_Y, 200), (0, 0, 0)]
        );
    }

    #[test]
    fn desktops() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        if cfg!(target_os = "linux") {
            assert_eq!(
                detect(&env(&[
                    ("XDG_SESSION_TYPE", "wayland"),
                    ("XDG_CURRENT_DESKTOP", "KDE")
                ]))
                .0,
                Some("kwin")
            );
            assert_eq!(
                detect(&env(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")])).0,
                Some("x11")
            );
            let (kind, why) = detect(&env(&[
                ("XDG_SESSION_TYPE", "wayland"),
                ("XDG_CURRENT_DESKTOP", "GNOME"),
            ]));
            assert!(kind.is_none());
            let why = why.unwrap();
            glisa_i18n::with_language(Some("es"), || {
                assert!(
                    why.render()
                        .contains("no está disponible en este escritorio"),
                    "{}",
                    why.render()
                );
                assert!(why.render().contains("GNOME"));
            });
            assert!(
                why.english()
                    .starts_with("Flow is not available on this desktop: on Wayland")
            );
            assert!(detect(&env(&[])).0.is_none());
        }
    }
}
