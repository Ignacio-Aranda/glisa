//! The edge catcher (KDE Plasma on Wayland): a `zwlr_layer_shell_v1` surface on the screen
//! edge that leads to a peer, up only while a mouse button is held there
//! (docs/FLOW-DRAG-DROP.md). It is [`GLOW`] pixels thick but only its outer pixel takes part
//! in drag and drop (its input region), so it never covers anything: it stays fully
//! transparent until files are dragged onto it, then shows a soft Iris glow along the edge
//! ([`glow_pixels`]) until the drag leaves it.
//!
//! It takes part in drag and drop through `wl_data_device`, nothing else: no pointer or
//! keyboard is bound and it never has keyboard focus, so it can't take an ordinary click
//! (a press that started elsewhere keeps its implicit grab). When a drag enters it:
//!
//! * the offer has `text/uri-list` → it is read through a pipe (at most
//!   [`URI_LIST_MAX`] bytes, within [`URI_LIST_TIMEOUT`]); the local paths
//!   ([`parse_drag_uris`]) are reported. The drop is not accepted yet: only once the
//!   engine says the crossing goes ahead ([`Catcher::accept`]) is it accepted, with the
//!   **copy** action only, so the drop the switch causes (the compositor lets go of the
//!   button when the mouse leaves) finishes as a copy; otherwise it is cancelled. Never
//!   waits for anything when the drop comes.
//! * anything else → refused.
//!
//! The same connection serves the drag watch (`dragwatch.rs`), which makes the edges glow
//! while files are dragged anywhere:
//!
//! * the **probe** ([`Catcher::probe`]): invisible surfaces over every output (a 1×1
//!   transparent buffer scaled with `wp_viewporter`; the whole surface takes input), up for a
//!   moment to see whether a drag and drop is going on and whether it carries files. Only the
//!   offer's MIME types are looked at: a drag entering it is never accepted, no data is
//!   received, and the probe is destroyed at once, so the drag goes on to whatever is under
//!   the pointer at its next motion. A drop on it (the button let go in that very instant)
//!   is refused, which cancels that drag. An ordinary press (a selection, a window being
//!   moved) keeps its implicit grab and never enters it.
//! * the **glow** ([`Catcher::glow`]): the catcher's Iris glow ([`glow_pixels`]) along every
//!   edge that leads to a peer, while the drag lasts. Visual only: its input region is empty,
//!   so it never takes part in input or drag and drop.
//!
//! On the computer the files go to, a second connection made with [`Catcher::for_pickup`]
//! (the only one with a `wl_pointer`) lets the drag go on there: the **pickup**
//! ([`Catcher::pickup_expect`]) is an invisible surface over the output where the pointer
//! will enter, up from the `switch` until the mouse's press (or a few seconds without one).
//! A left press on it, once the engine says our mouse is here and the mouse's own evdev node
//! confirms its left button is down ([`PickupHooks::confirm`]), starts a drag with that
//! press's serial: `text/uri-list` of the received files, copy only, a small Iris icon
//! ([`pickup_icon`]). The pickup then takes no input, so the drag goes wherever the user
//! takes it; a drop target's `send` gets the list once every file is here (at most
//! [`SEND_WAIT`] later; otherwise nothing, and the files go on the clipboard instead). The
//! drag may offer more MIME types with fixed contents ([`PickupExtra`]): KDE's way of
//! copying without its Copy/Link menu (`arkdrop.rs`).
//!
//! Wire-level client like the rest of `linux/` (`wayland.rs`): wl_compositor, wl_shm (one
//! zero-filled memfd buffer = transparent ARGB), zwlr_layer_shell_v1 (overlay layer,
//! anchored to the edge, exclusive zone −1), wl_data_device_manager v3,
//! zxdg_output_manager_v1 to put the surface on the output under the pointer, and
//! wp_viewporter for the probe.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::wayland::{self, DISPLAY, Message, Sender, Wire, string, u32s};
use crate::backends::PickupNews;
use crate::drag::{URI_LIST_TIMEOUT, parse_drag_uris};
use crate::geometry::{GlowEdge, Rect};
use crate::transfer::URI_LIST_MAX;
use crate::util::lock;

/// Reports files dragged onto the catcher: (side, local paths).
pub type OnDrag = Arc<dyn Fn(&'static str, Vec<PathBuf>) + Send + Sync>;

const REGISTRY: u32 = 2;
pub const URI_LIST: &str = "text/uri-list";
const NAMESPACE: &str = "glisa-flow-catcher";
const PROBE_NAMESPACE: &str = "glisa-flow-probe";
const GLOW_NAMESPACE: &str = "glisa-flow-glow";
const PICKUP_NAMESPACE: &str = "glisa-flow-pickup";
/// How long a press on the pickup may wait for the mouse to confirm its left button.
const CONFIRM_WAIT: Duration = Duration::from_millis(400);
/// How long a drop target's `send` waits for the files to be all here.
pub const SEND_WAIT: Duration = Duration::from_secs(1);
const BTN_LEFT: u32 = 0x110;
/// A catcher taken away while a drag is on it goes once the drag is over, or after this.
const LINGER: Duration = Duration::from_secs(15);
/// How long the probe waits for the compositor to put it on screen.
const PROBE_MAP_WAIT: Duration = Duration::from_millis(250);

// wl_display / wl_registry / wl_callback
const EV_DISPLAY_ERROR: u16 = 0;
const EV_GLOBAL: u16 = 0;
const EV_GLOBAL_REMOVE: u16 = 1;
const EV_CALLBACK_DONE: u16 = 0;
// wl_compositor, wl_surface, wl_region, wl_shm, wl_shm_pool, wl_buffer
const COMPOSITOR_CREATE_SURFACE: u16 = 0;
const COMPOSITOR_CREATE_REGION: u16 = 1;
const REGION_DESTROY: u16 = 0;
const REGION_ADD: u16 = 1;
const SURFACE_DESTROY: u16 = 0;
const SURFACE_ATTACH: u16 = 1;
const SURFACE_DAMAGE: u16 = 2;
const SURFACE_SET_INPUT_REGION: u16 = 5;
const SURFACE_COMMIT: u16 = 6;
const SURFACE_DAMAGE_BUFFER: u16 = 9;
const SHM_CREATE_POOL: u16 = 0;
const POOL_CREATE_BUFFER: u16 = 0;
const POOL_DESTROY: u16 = 1;
const BUFFER_DESTROY: u16 = 0;
const FORMAT_ARGB8888: u32 = 0;
// zwlr_layer_shell_v1 / zwlr_layer_surface_v1
const LAYER_SHELL_GET_LAYER_SURFACE: u16 = 0;
const LAYER_OVERLAY: u32 = 3;
const LS_SET_SIZE: u16 = 0;
const LS_SET_ANCHOR: u16 = 1;
const LS_SET_EXCLUSIVE_ZONE: u16 = 2;
const LS_SET_MARGIN: u16 = 3;
const LS_SET_KEYBOARD_INTERACTIVITY: u16 = 4;
const LS_ACK_CONFIGURE: u16 = 6;
const LS_DESTROY: u16 = 7;
const EV_LS_CONFIGURE: u16 = 0;
const EV_LS_CLOSED: u16 = 1;
const ANCHOR_TOP: u32 = 1;
const ANCHOR_BOTTOM: u32 = 2;
const ANCHOR_LEFT: u32 = 4;
const ANCHOR_RIGHT: u32 = 8;
const ANCHOR_ALL: u32 = ANCHOR_TOP | ANCHOR_BOTTOM | ANCHOR_LEFT | ANCHOR_RIGHT;
// wl_seat / wl_pointer
const SEAT_GET_POINTER: u16 = 0;
const EV_PTR_ENTER: u16 = 0;
const EV_PTR_LEAVE: u16 = 1;
const EV_PTR_BUTTON: u16 = 3;
// wp_cursor_shape_manager_v1 / wp_cursor_shape_device_v1
const SHAPE_GET_POINTER: u16 = 1;
const SHAPE_SET_SHAPE: u16 = 1;
const SHAPE_DEFAULT: u32 = 1;
// wl_data_device_manager / wl_data_device / wl_data_source / wl_data_offer
const DDM_CREATE_DATA_SOURCE: u16 = 0;
const DDM_GET_DATA_DEVICE: u16 = 1;
const DD_START_DRAG: u16 = 0;
const SOURCE_OFFER: u16 = 0;
const SOURCE_DESTROY: u16 = 1;
const SOURCE_SET_ACTIONS: u16 = 2;
const EV_SOURCE_TARGET: u16 = 0;
const EV_SOURCE_SEND: u16 = 1;
const EV_SOURCE_CANCELLED: u16 = 2;
const EV_SOURCE_DROP_PERFORMED: u16 = 3;
const EV_SOURCE_FINISHED: u16 = 4;
const EV_SOURCE_ACTION: u16 = 5;
const EV_DD_DATA_OFFER: u16 = 0;
const EV_DD_ENTER: u16 = 1;
const EV_DD_LEAVE: u16 = 2;
const EV_DD_DROP: u16 = 4;
const EV_DD_SELECTION: u16 = 5;
const OFFER_ACCEPT: u16 = 0;
const OFFER_RECEIVE: u16 = 1;
const OFFER_DESTROY: u16 = 2;
const OFFER_FINISH: u16 = 3;
const OFFER_SET_ACTIONS: u16 = 4;
const EV_OFFER_OFFER: u16 = 0;
const DND_COPY: u32 = 1;
// zxdg_output_manager_v1 / zxdg_output_v1
const XDG_GET_OUTPUT: u16 = 1;
const EV_XDG_POSITION: u16 = 0;
const EV_XDG_SIZE: u16 = 1;
// wp_viewporter / wp_viewport
const VIEWPORTER_GET_VIEWPORT: u16 = 1;
const VIEWPORT_DESTROY: u16 = 0;
const VIEWPORT_SET_DESTINATION: u16 = 2;

/// Thickness of the catcher (and of the glow it shows), in logical pixels.
pub const GLOW: u32 = 120;

/// Anchor bits and requested size of the catcher on `side`.
pub fn placement(side: &str) -> (u32, u32, u32) {
    match side {
        "left" => (ANCHOR_TOP | ANCHOR_BOTTOM | ANCHOR_LEFT, GLOW, 0),
        "right" => (ANCHOR_TOP | ANCHOR_BOTTOM | ANCHOR_RIGHT, GLOW, 0),
        "top" => (ANCHOR_LEFT | ANCHOR_RIGHT | ANCHOR_TOP, 0, GLOW),
        _ => (ANCHOR_LEFT | ANCHOR_RIGHT | ANCHOR_BOTTOM, 0, GLOW),
    }
}

/// Anchor bits, requested size and margins (top, right, bottom, left) of the glow strip for
/// `e`: the whole side of its output like the catcher, or the stretch of it that is outer.
pub fn glow_placement(e: &GlowEdge) -> (u32, u32, u32, [i32; 4]) {
    let vertical = matches!(e.side, "left" | "right");
    let length = if vertical { e.output[3] } else { e.output[2] };
    if e.start <= 0 && e.len >= length {
        let (anchor, w, h) = placement(e.side);
        return (anchor, w, h, [0; 4]);
    }
    let (start, len) = (e.start.max(0) as i32, e.len.max(1) as u32);
    match e.side {
        "left" => (ANCHOR_LEFT | ANCHOR_TOP, GLOW, len, [start, 0, 0, 0]),
        "right" => (ANCHOR_RIGHT | ANCHOR_TOP, GLOW, len, [start, 0, 0, 0]),
        "top" => (ANCHOR_TOP | ANCHOR_LEFT, len, GLOW, [0, 0, 0, start]),
        _ => (ANCHOR_BOTTOM | ANCHOR_LEFT, len, GLOW, [0, 0, 0, start]),
    }
}

/// The part of the surface that takes part in drag and drop: its outermost pixel along the
/// edge (x, y, w, h in surface coordinates).
pub fn input_strip(side: &str) -> (i32, i32, i32, i32) {
    const LONG: i32 = 1 << 20;
    let edge = GLOW as i32 - 1;
    match side {
        "left" => (0, 0, 1, LONG),
        "right" => (edge, 0, 1, LONG),
        "top" => (0, 0, LONG, 1),
        _ => (0, edge, LONG, 1),
    }
}

/// The glow for files dragged onto the catcher on `side`, as premultiplied ARGB8888 pixels
/// (`w`×`h`, little-endian): an Iris shadow that fades away from the edge, a thin light line
/// on the edge itself with a little bloom, both fading out towards the ends of the edge.
pub fn glow_pixels(side: &str, w: u32, h: u32) -> Vec<u8> {
    const SHADOW: (f32, f32, f32) = (122.0, 102.0, 255.0); // Iris
    const LINE: (f32, f32, f32) = (200.0, 192.0, 255.0);
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let vertical = matches!(side, "left" | "right");
    let (depth, length) = if vertical { (w, h) } else { (h, w) };
    let smooth = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    // Per distance from the edge: (shadow alpha, line alpha).
    let across: Vec<(f32, f32)> = (0..depth)
        .map(|d| {
            let d = d as f32;
            let fade = (1.0 - d / depth.max(1) as f32).max(0.0);
            let shadow = 0.34 * (-d / 36.0).exp() * fade;
            let line = if d < 1.5 { 0.85 } else { 0.0 } + 0.45 * (-d / 4.0).exp();
            (shadow, line.min(1.0))
        })
        .collect();
    // Along the edge: fade in and out over the first and last 18 %.
    let along: Vec<f32> = (0..length)
        .map(|i| {
            let t = (i as f32 + 0.5) / length.max(1) as f32;
            smooth(0.0, 0.18, t) * smooth(0.0, 0.18, 1.0 - t)
        })
        .collect();
    for y in 0..h {
        for x in 0..w {
            let d = match side {
                "left" => x,
                "right" => w - 1 - x,
                "top" => y,
                _ => h - 1 - y,
            };
            let (a_shadow, a_line) = across[d];
            let m = along[if vertical { y } else { x }];
            let (a1, a2) = (a_shadow * m, a_line * m);
            let a = a2 + a1 * (1.0 - a2);
            let ch =
                |c1: f32, c2: f32| (c2 * a2 + c1 * a1 * (1.0 - a2)).round().clamp(0.0, 255.0) as u8;
            let i = (y * w + x) * 4;
            out[i] = ch(SHADOW.2, LINE.2); // B
            out[i + 1] = ch(SHADOW.1, LINE.1); // G
            out[i + 2] = ch(SHADOW.0, LINE.0); // R
            out[i + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8; // A
        }
    }
    out
}

#[derive(Debug, Default, Clone, Copy)]
struct Ids {
    compositor: u32,
    compositor_version: u32,
    shm: u32,
    layer_shell: u32,
    seat: u32,
    ddm: u32,
    device: u32,
    xdg_manager: u32,
    viewporter: u32,
    /// The pickup's wl_pointer, and its cursor-shape device (0: none).
    pointer: u32,
    cursor_shape: u32,
    shape_device: u32,
}

#[derive(Debug, Clone)]
struct Output {
    global: u32,
    id: u32,
    xdg: u32,
    pos: (i64, i64),
    size: (i64, i64),
}

#[derive(Debug)]
struct Surface {
    side: &'static str,
    rect: Rect,
    surface: u32,
    layer: u32,
    /// The transparent buffer (id, w, h): what the catcher shows while idle.
    buffer: Option<(u32, u32, u32)>,
    /// The glow buffer (id, w, h), made the first time files are dragged onto it.
    glow: Option<(u32, u32, u32)>,
    /// The glow is on screen.
    lit: bool,
    linger_until: Option<Instant>,
}

#[derive(Debug)]
struct Dnd {
    offer: u32,
    serial: u32,
    files: bool,
    accepted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Probe,
    Glow,
    Pickup,
}

/// A probe or glow surface.
#[derive(Debug)]
struct Extra {
    role: Role,
    side: &'static str,
    surface: u32,
    layer: u32,
    /// Its wp_viewport (probes), or 0.
    viewport: u32,
    /// What it shows (id, w, h) once configured; a probe's is [`Probe::buffer`].
    buffer: Option<(u32, u32, u32)>,
}

/// The probe going on.
#[derive(Debug)]
struct Probe {
    /// The 1×1 transparent buffer every probe surface shows (0: destroyed).
    buffer: u32,
    /// Probe surfaces not on screen yet.
    unmapped: usize,
    /// What entered it: Some(it offers files).
    found: Option<bool>,
}

/// A drag from another computer that may go on here.
#[derive(Debug)]
struct PickupState {
    /// Items coming (for the icon).
    count: usize,
    /// The 1×1 transparent buffer of its surface (0: destroyed).
    buffer: u32,
    /// Our mouse is here: its press counts.
    armed: bool,
    /// The left press being checked (its serial), while the button is down.
    press: Option<u32>,
    /// The drag: its data source, icon surface and icon buffer (0: not started).
    source: u32,
    icon: u32,
    icon_buffer: u32,
    /// The URI list a drop gets, once every file is here.
    uris: Option<Vec<u8>>,
    /// A drop target got the list.
    delivered: bool,
    /// More MIME types the drag offers.
    extra: PickupExtra,
}

/// More MIME types the drag picked up here offers besides `text/uri-list` (KDE's way of
/// copying without asking, `arkdrop.rs`), each with what a drop target gets for it: fixed,
/// written at once, never waited for.
#[derive(Clone, Default)]
pub struct PickupExtra {
    pub offers: Vec<(String, Vec<u8>)>,
    /// Told which one a drop target asked for, before it is written.
    pub on_send: Option<OnSend>,
}

/// See [`PickupExtra::on_send`].
pub type OnSend = Arc<dyn Fn(&str) + Send + Sync>;

impl std::fmt::Debug for PickupExtra {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let types: Vec<&str> = self.offers.iter().map(|(m, _)| m.as_str()).collect();
        f.debug_struct("PickupExtra")
            .field("offers", &types)
            .finish()
    }
}

/// What the pickup asks and tells (the KWin backend wires them to the mouse and the engine).
#[derive(Clone)]
pub struct PickupHooks {
    /// Is our mouse's left button down right now (so the press is its own)?
    pub confirm: Arc<dyn Fn() -> bool + Send + Sync>,
    /// How the drag went.
    pub news: Arc<dyn Fn(PickupNews) + Send + Sync>,
}

/// What happened, for tests and logs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counters {
    pub shown: u32,
    pub mapped: u32,
    pub removed: u32,
    pub entered: u32,
    pub reported: u32,
    pub dropped: u32,
    pub finished: u32,
    /// Times the glow was put on screen.
    pub lit: u32,
    /// Probes put up; probe surfaces on screen; what the probes found: files, another
    /// drag, nothing.
    pub probes: u32,
    pub probe_mapped: u32,
    pub probe_files: u32,
    pub probe_other: u32,
    pub probe_missed: u32,
    /// Drops that landed on a probe (refused).
    pub probe_drops: u32,
    /// Glow strips put on screen, and taken away.
    pub glows: u32,
    pub glows_removed: u32,
    /// Pickups put up and on screen; left presses on one; drags started; lists handed to a
    /// drop target; drops performed.
    pub pickups: u32,
    pub pickup_mapped: u32,
    pub pickup_presses: u32,
    pub pickup_started: u32,
    pub pickup_sent: u32,
    pub pickup_drops: u32,
    /// Extra MIME types ([`PickupExtra`]) handed to a drop target.
    pub pickup_extra_sent: u32,
}

struct State {
    alive: bool,
    next_id: u32,
    ids: Ids,
    outputs: Vec<Output>,
    offers: HashMap<u32, Vec<String>>,
    surface: Option<Surface>,
    dnd: Option<Dnd>,
    /// The drag is on a surface of ours that isn't the catcher (a probe, or one already
    /// gone): its leave or drop concerns nothing here.
    elsewhere: bool,
    /// Probe and glow surfaces.
    extras: Vec<Extra>,
    probe: Option<Probe>,
    /// Where the glow is asked for.
    glow: Vec<GlowEdge>,
    pickup: Option<PickupState>,
    /// The surface under the pointer (pickup connection).
    pointer_focus: u32,
    /// Bumped at every drag entering: a late uri-list of an earlier one is ignored.
    enter_seq: u64,
    callbacks: HashMap<u32, bool>,
    counters: Counters,
}

impl State {
    fn new_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The output whose logical geometry is `rect` (else the one holding its centre).
    fn output_for(&self, rect: Rect) -> u32 {
        let r = |o: &Output| [o.pos.0, o.pos.1, o.size.0, o.size.1];
        if let Some(o) = self.outputs.iter().find(|o| r(o) == rect) {
            return o.id;
        }
        let (cx, cy) = (rect[0] + rect[2] / 2, rect[1] + rect[3] / 2);
        if let Some(o) = self.outputs.iter().find(|o| {
            let [x, y, w, h] = r(o);
            w > 0 && h > 0 && cx >= x && cx < x + w && cy >= y && cy < y + h
        }) {
            return o.id;
        }
        if self.outputs.len() == 1 {
            return self.outputs[0].id;
        }
        0 // the compositor chooses
    }
}

type Shared = Arc<(Mutex<State>, Condvar)>;

pub struct Catcher {
    st: Shared,
    sender: Mutex<Option<Arc<Sender>>>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    on_drag: OnDrag,
    /// The pickup's connection (it alone binds a pointer).
    pickup_hooks: Option<PickupHooks>,
    pub error: Arc<Mutex<String>>,
}

impl Catcher {
    /// The connection for the computer the files go to: the pickup.
    pub fn for_pickup(hooks: PickupHooks) -> Self {
        let mut c = Self::new(Arc::new(|_, _| {}));
        c.pickup_hooks = Some(hooks);
        c
    }

    pub fn new(on_drag: OnDrag) -> Self {
        Self {
            st: Arc::new((
                Mutex::new(State {
                    alive: false,
                    next_id: 3,
                    ids: Ids::default(),
                    outputs: Vec::new(),
                    offers: HashMap::new(),
                    surface: None,
                    dnd: None,
                    elsewhere: false,
                    extras: Vec::new(),
                    probe: None,
                    glow: Vec::new(),
                    pickup: None,
                    pointer_focus: 0,
                    enter_seq: 0,
                    callbacks: HashMap::new(),
                    counters: Counters::default(),
                }),
                Condvar::new(),
            )),
            sender: Mutex::new(None),
            stop: Arc::default(),
            thread: Mutex::new(None),
            on_drag,
            pickup_hooks: None,
            error: Arc::default(),
        }
    }

    fn sender(&self) -> Option<Arc<Sender>> {
        lock(&self.sender).clone()
    }

    pub fn available(&self) -> bool {
        lock(&self.st.0).alive
    }

    pub fn counters(&self) -> Counters {
        lock(&self.st.0).counters.clone()
    }

    /// Is a catcher on screen (configured and given its buffer)?
    pub fn mapped(&self) -> bool {
        lock(&self.st.0)
            .surface
            .as_ref()
            .is_some_and(|s| s.buffer.is_some())
    }

    /// Connects (to `sock`, or the session's compositor). False, with `error`, when the
    /// compositor lacks something the catcher needs.
    pub fn start(&self, sock: Option<UnixStream>) -> bool {
        let timeout = Duration::from_secs(2);
        let result = (|| -> std::io::Result<Result<Wire, String>> {
            let sock = match sock {
                Some(s) => s,
                None => wayland::connect(timeout)?,
            };
            let mut wire = Wire::new(sock);
            let sync = lock(&self.st.0).new_id();
            let globals = initial_globals(&mut wire, sync, timeout)?;
            let find = |iface: &str| globals.iter().find(|g| g.1 == iface).cloned();
            let (Some(comp), Some(shm), Some(layer), Some(seat), Some(ddm)) = (
                find("wl_compositor"),
                find("wl_shm"),
                find("zwlr_layer_shell_v1"),
                find("wl_seat"),
                find("wl_data_device_manager"),
            ) else {
                return Ok(Err(
                    "the compositor doesn't offer layer-shell surfaces".into()
                ));
            };
            if ddm.2 < 3 {
                return Ok(Err("the compositor's drag and drop is too old".into()));
            }
            let xdg = find("zxdg_output_manager_v1");
            let viewporter = find("wp_viewporter");
            let pickup = self.pickup_hooks.is_some();
            let shapes = find("wp_cursor_shape_manager_v1").filter(|_| pickup);
            let mut st = lock(&self.st.0);
            let ids = Ids {
                compositor: st.new_id(),
                compositor_version: comp.2.min(4),
                shm: st.new_id(),
                layer_shell: st.new_id(),
                seat: st.new_id(),
                ddm: st.new_id(),
                device: st.new_id(),
                xdg_manager: if xdg.is_some() { st.new_id() } else { 0 },
                viewporter: if viewporter.is_some() { st.new_id() } else { 0 },
                pointer: if pickup { st.new_id() } else { 0 },
                cursor_shape: if shapes.is_some() { st.new_id() } else { 0 },
                shape_device: if shapes.is_some() { st.new_id() } else { 0 },
            };
            wayland::bind(
                &wire,
                REGISTRY,
                comp.0,
                "wl_compositor",
                ids.compositor_version,
                ids.compositor,
            )?;
            wayland::bind(&wire, REGISTRY, shm.0, "wl_shm", 1, ids.shm)?;
            wayland::bind(
                &wire,
                REGISTRY,
                layer.0,
                "zwlr_layer_shell_v1",
                layer.2.min(3),
                ids.layer_shell,
            )?;
            wayland::bind(&wire, REGISTRY, seat.0, "wl_seat", 1, ids.seat)?;
            wayland::bind(&wire, REGISTRY, ddm.0, "wl_data_device_manager", 3, ids.ddm)?;
            wire.send(ids.ddm, DDM_GET_DATA_DEVICE, &u32s(&[ids.device, ids.seat]))?;
            if let Some(x) = &xdg {
                wayland::bind(
                    &wire,
                    REGISTRY,
                    x.0,
                    "zxdg_output_manager_v1",
                    x.2.min(3),
                    ids.xdg_manager,
                )?;
            }
            if let Some(v) = &viewporter {
                wayland::bind(&wire, REGISTRY, v.0, "wp_viewporter", 1, ids.viewporter)?;
            }
            if ids.pointer != 0 {
                wire.send(ids.seat, SEAT_GET_POINTER, &u32s(&[ids.pointer]))?;
            }
            if let Some(c) = &shapes {
                let name = "wp_cursor_shape_manager_v1";
                wayland::bind(&wire, REGISTRY, c.0, name, 1, ids.cursor_shape)?;
                let ids2 = [ids.shape_device, ids.pointer];
                wire.send(ids.cursor_shape, SHAPE_GET_POINTER, &u32s(&ids2))?;
            }
            st.ids = ids;
            for g in globals.iter().filter(|g| g.1 == "wl_output") {
                let o = add_output(&mut st, g.0, g.2);
                bind_output(&wire, &st, &o)?;
                st.outputs.push(o);
            }
            st.alive = true;
            Ok(Ok(wire))
        })();
        let wire = match result {
            Ok(Ok(w)) => w,
            Ok(Err(why)) => {
                *lock(&self.error) = why;
                return false;
            }
            Err(e) => {
                *lock(&self.error) = e.to_string();
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
        let hooks = Hooks {
            on_drag: self.on_drag.clone(),
            pickup: self.pickup_hooks.clone(),
        };
        let t = std::thread::Builder::new()
            .name("flow-catcher-wl".into())
            .spawn(move || run(wire, sender, st, stop, error, hooks));
        *lock(&self.thread) = t.ok();
        self.sync(Duration::from_secs(2)); // the outputs' geometry is known
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
        let cb = st.new_id();
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

    /// Puts the catcher on `side` of the output whose logical geometry is `rect`
    /// (moving it if it is elsewhere, unless a drag is on it).
    pub fn show(&self, side: &'static str, rect: Rect) -> bool {
        let Some(sender) = self.sender() else {
            return false;
        };
        let mut st = lock(&self.st.0);
        if !st.alive {
            return false;
        }
        if let Some(s) = st.surface.as_mut() {
            if s.side == side && s.rect == rect {
                s.linger_until = None;
                return true;
            }
            if st.dnd.is_some() {
                return false; // can't move it from under a drag
            }
            destroy_surface(&sender, &mut st);
        }
        let output = st.output_for(rect);
        let (anchor, w, h) = placement(side);
        let (surface, layer, _, sent) = new_layer(
            &sender,
            &mut st,
            &LayerSpec {
                output,
                namespace: NAMESPACE,
                anchor,
                size: (w, h),
                margin: [0; 4],
                // Only the outer pixel takes part in drag and drop: the rest is just the glow.
                input: Some(&[input_strip(side)]),
                viewport: false,
            },
        );
        st.surface = Some(Surface {
            side,
            rect,
            surface,
            layer,
            buffer: None,
            glow: None,
            lit: false,
            linger_until: None,
        });
        st.counters.shown += 1;
        sent.is_ok()
    }

    /// Takes the catcher away (once the drag on it, if any, is over).
    pub fn hide(&self) {
        let Some(sender) = self.sender() else {
            return;
        };
        let mut st = lock(&self.st.0);
        if st.surface.is_none() {
            return;
        }
        if st.dnd.is_some() {
            if let Some(s) = st.surface.as_mut() {
                s.linger_until.get_or_insert(Instant::now() + LINGER);
            }
            return;
        }
        destroy_surface(&sender, &mut st);
    }

    /// The crossing for the files on the catcher goes ahead (accept their drop, as a
    /// copy) or not (refuse it). False when no file drag is on the catcher.
    pub fn accept(&self, yes: bool) -> bool {
        let Some(sender) = self.sender() else {
            return false;
        };
        let mut st = lock(&self.st.0);
        let Some(d) = st.dnd.as_mut().filter(|d| d.files) else {
            return false;
        };
        let mut p = u32s(&[d.serial]);
        if yes {
            p.extend(string(URI_LIST));
        } else {
            p.extend(u32s(&[0])); // null: nothing accepted
        }
        if sender.send(d.offer, OFFER_ACCEPT, &p).is_err() {
            return false;
        }
        d.accepted = yes;
        true
    }

    /// The drag watch's probe: invisible surfaces over every output, then at most `window`
    /// (once they are on screen) for a drag to enter one. Some(true): a drag offering
    /// `text/uri-list` (files); Some(false): another drag; None: nothing entered (no drag
    /// going on, or no probe possible here). The probe is gone when this returns.
    pub fn probe(&self, window: Duration) -> Option<bool> {
        let sender = self.sender()?;
        let (m, cv) = &*self.st;
        let mut st = lock(m);
        // Not while a drag is on the catcher: the probe would take it from there.
        if !st.alive || st.ids.viewporter == 0 || st.probe.is_some() || st.dnd.is_some() {
            return None;
        }
        let buffer = shm_buffer(&sender, &mut st, 1, 1, None).ok()?;
        let mut outputs: Vec<u32> = st.outputs.iter().map(|o| o.id).collect();
        if outputs.is_empty() {
            outputs.push(0); // the compositor chooses
        }
        st.probe = Some(Probe {
            buffer,
            unmapped: outputs.len(),
            found: None,
        });
        st.counters.probes += 1;
        for output in outputs {
            let (surface, layer, viewport, sent) = new_layer(
                &sender,
                &mut st,
                &LayerSpec {
                    output,
                    namespace: PROBE_NAMESPACE,
                    anchor: ANCHOR_ALL,
                    size: (0, 0),
                    margin: [0; 4],
                    input: None, // the whole surface
                    viewport: true,
                },
            );
            st.extras.push(Extra {
                role: Role::Probe,
                side: "",
                surface,
                layer,
                viewport,
                buffer: None,
            });
            if sent.is_err() {
                break;
            }
        }
        // On screen (or as good as it gets)…
        st = wait_probe(cv, st, Instant::now() + PROBE_MAP_WAIT, |p| p.unmapped > 0);
        // …then a drag going on enters it at its next motion.
        st = wait_probe(cv, st, Instant::now() + window, |_| true);
        remove_probe(&sender, &mut st);
        let found = st.probe.take().and_then(|p| p.found);
        if found.is_none() {
            st.counters.probe_missed += 1;
        }
        found
    }

    /// Shows the drag glow along `edges` (empty: takes it away). Visual only: the strips'
    /// input region is empty, so they never take part in input or drag and drop.
    pub fn glow(&self, edges: &[GlowEdge]) -> bool {
        let Some(sender) = self.sender() else {
            return false;
        };
        let mut st = lock(&self.st.0);
        if !st.alive {
            return false;
        }
        if st.glow == edges {
            return true;
        }
        remove_glow(&sender, &mut st);
        st.glow = edges.to_vec();
        let mut ok = true;
        for e in edges {
            let output = st.output_for(e.output);
            let (anchor, w, h, margin) = glow_placement(e);
            let (surface, layer, _, sent) = new_layer(
                &sender,
                &mut st,
                &LayerSpec {
                    output,
                    namespace: GLOW_NAMESPACE,
                    anchor,
                    size: (w, h),
                    margin,
                    input: Some(&[]), // nothing: never in the way
                    viewport: false,
                },
            );
            st.extras.push(Extra {
                role: Role::Glow,
                side: e.side,
                surface,
                layer,
                viewport: 0,
                buffer: None,
            });
            ok &= sent.is_ok();
        }
        ok
    }

    /// Glow strips on screen now.
    pub fn glowing(&self) -> usize {
        lock(&self.st.0)
            .extras
            .iter()
            .filter(|x| x.role == Role::Glow && x.buffer.is_some())
            .count()
    }

    /// Can a drag from another computer go on here (a pickup connection with everything
    /// it needs)?
    pub fn pickup_possible(&self) -> bool {
        let st = lock(&self.st.0);
        st.alive && st.ids.pointer != 0 && st.ids.viewporter != 0
    }

    /// A drag from another computer is coming: an invisible surface over the output `rect`
    /// waits for the arriving mouse's press (`count` items, for the drag's icon; `extra`,
    /// what the drag offers besides the URI list). Replaces a pickup still waiting; false
    /// while a drag picked up here is going on.
    pub fn pickup_expect(&self, rect: Rect, count: usize, extra: PickupExtra) -> bool {
        let Some(sender) = self.sender() else {
            return false;
        };
        let mut st = lock(&self.st.0);
        if !st.alive || st.ids.pointer == 0 || st.ids.viewporter == 0 {
            return false;
        }
        if st.pickup.as_ref().is_some_and(|p| p.source != 0) {
            return false;
        }
        remove_pickup(&sender, &mut st);
        let Ok(buffer) = shm_buffer(&sender, &mut st, 1, 1, None) else {
            return false;
        };
        st.pickup = Some(PickupState {
            count,
            buffer,
            armed: false,
            press: None,
            source: 0,
            icon: 0,
            icon_buffer: 0,
            uris: None,
            delivered: false,
            extra,
        });
        let output = st.output_for(rect);
        let (surface, layer, viewport, sent) = new_layer(
            &sender,
            &mut st,
            &LayerSpec {
                output,
                namespace: PICKUP_NAMESPACE,
                anchor: ANCHOR_ALL,
                size: (0, 0),
                margin: [0; 4],
                input: None, // the whole output: the press lands here wherever it comes
                viewport: true,
            },
        );
        st.extras.push(Extra {
            role: Role::Pickup,
            side: "",
            surface,
            layer,
            viewport,
            buffer: None,
        });
        st.counters.pickups += 1;
        sent.is_ok()
    }

    /// Our mouse is here: a left press on the pickup now starts the drag.
    pub fn pickup_arrived(&self) {
        if let Some(p) = lock(&self.st.0).pickup.as_mut() {
            p.armed = true;
        }
    }

    /// Every file is here: what a drop gets (`text/uri-list`).
    pub fn pickup_ready(&self, uris: Vec<u8>) {
        if let Some(p) = lock(&self.st.0).pickup.as_mut() {
            p.uris = Some(uris);
        }
        self.st.1.notify_all();
    }

    /// The pickup is over: its surface goes, and a drag still going on is cancelled.
    pub fn pickup_cancel(&self) {
        let Some(sender) = self.sender() else {
            return;
        };
        remove_pickup(&sender, &mut lock(&self.st.0));
        self.st.1.notify_all();
    }

    /// Is the pickup on screen, waiting for the press?
    pub fn pickup_waiting(&self) -> bool {
        let st = lock(&self.st.0);
        st.pickup.as_ref().is_some_and(|p| p.source == 0)
            && st
                .extras
                .iter()
                .any(|x| x.role == Role::Pickup && x.buffer.is_some())
    }

    /// Is a drag picked up here going on?
    pub fn pickup_dragging(&self) -> bool {
        lock(&self.st.0)
            .pickup
            .as_ref()
            .is_some_and(|p| p.source != 0)
    }

    pub fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(s) = lock(&self.sender).take() {
            s.shutdown();
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

impl Drop for Catcher {
    fn drop(&mut self) {
        self.close();
    }
}

/// Every global of the first registry round trip: (name, interface, version).
fn initial_globals(
    wire: &mut Wire,
    sync: u32,
    timeout: Duration,
) -> std::io::Result<Vec<(u32, String, u32)>> {
    wire.send(DISPLAY, 1, &u32s(&[REGISTRY]))?; // get_registry
    wire.send(DISPLAY, 0, &u32s(&[sync]))?; // sync
    let mut out = Vec::new();
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        for m in wire.read(Duration::from_millis(50))? {
            if m.object == REGISTRY && m.opcode == EV_GLOBAL {
                let name = wayland::read_u32(&m.body, 0)?;
                let (iface, off) = wayland::read_string(&m.body, 4)?;
                let version = wayland::read_u32(&m.body, off)?;
                if out.len() < 256 {
                    out.push((name, iface, version));
                }
            } else if m.object == sync {
                return Ok(out);
            } else if m.object == DISPLAY && m.opcode == EV_DISPLAY_ERROR {
                return Err(wayland::bad("Wayland protocol error"));
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "the compositor didn't answer",
    ))
}

fn add_output(st: &mut State, global: u32, _version: u32) -> Output {
    Output {
        global,
        id: st.new_id(),
        xdg: if st.ids.xdg_manager != 0 {
            st.new_id()
        } else {
            0
        },
        pos: (0, 0),
        size: (0, 0),
    }
}

fn bind_output(wire: &Wire, st: &State, o: &Output) -> std::io::Result<()> {
    wayland::bind(wire, REGISTRY, o.global, "wl_output", 1, o.id)?;
    if o.xdg != 0 {
        wire.send(st.ids.xdg_manager, XDG_GET_OUTPUT, &u32s(&[o.xdg, o.id]))?;
    }
    Ok(())
}

/// Waits while the probe has found nothing and `more` holds, until `until`.
fn wait_probe<'a>(
    cv: &Condvar,
    mut st: std::sync::MutexGuard<'a, State>,
    until: Instant,
    more: impl Fn(&Probe) -> bool,
) -> std::sync::MutexGuard<'a, State> {
    while st.alive
        && st
            .probe
            .as_ref()
            .is_some_and(|p| p.found.is_none() && more(p))
    {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        st = cv
            .wait_timeout(st, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    st
}

/// What a new layer surface is.
struct LayerSpec<'a> {
    output: u32,
    namespace: &'a str,
    anchor: u32,
    size: (u32, u32),
    /// top, right, bottom, left
    margin: [i32; 4],
    /// The input region: None, the whole surface (the default); Some, only these
    /// rectangles (x, y, w, h); Some(&[]), none at all.
    input: Option<&'a [(i32, i32, i32, i32)]>,
    /// With a wp_viewport (when the compositor has them).
    viewport: bool,
}

/// Creates an overlay layer surface (exclusive zone −1, no keyboard) and commits it so the
/// compositor configures it: (surface, layer surface, viewport or 0, whether it was sent).
/// Ids are created in order, under the state lock like every request that creates one.
fn new_layer(
    sender: &Sender,
    st: &mut State,
    spec: &LayerSpec,
) -> (u32, u32, u32, std::io::Result<()>) {
    let ids = st.ids;
    let surface = st.new_id();
    let viewport = if spec.viewport && ids.viewporter != 0 {
        st.new_id()
    } else {
        0
    };
    let layer = st.new_id();
    let region = if spec.input.is_some() { st.new_id() } else { 0 };
    let sent = (|| -> std::io::Result<()> {
        sender.send(ids.compositor, COMPOSITOR_CREATE_SURFACE, &u32s(&[surface]))?;
        if viewport != 0 {
            sender.send(
                ids.viewporter,
                VIEWPORTER_GET_VIEWPORT,
                &u32s(&[viewport, surface]),
            )?;
        }
        let mut p = u32s(&[layer, surface, spec.output, LAYER_OVERLAY]);
        p.extend(string(spec.namespace));
        sender.send(ids.layer_shell, LAYER_SHELL_GET_LAYER_SURFACE, &p)?;
        sender.send(layer, LS_SET_SIZE, &u32s(&[spec.size.0, spec.size.1]))?;
        sender.send(layer, LS_SET_ANCHOR, &u32s(&[spec.anchor]))?;
        if spec.margin != [0; 4] {
            let m: Vec<u8> = spec.margin.iter().flat_map(|v| v.to_ne_bytes()).collect();
            sender.send(layer, LS_SET_MARGIN, &m)?;
        }
        sender.send(layer, LS_SET_EXCLUSIVE_ZONE, &(-1i32).to_ne_bytes())?;
        sender.send(layer, LS_SET_KEYBOARD_INTERACTIVITY, &u32s(&[0]))?;
        if let Some(rects) = spec.input {
            sender.send(ids.compositor, COMPOSITOR_CREATE_REGION, &u32s(&[region]))?;
            for (x, y, w, h) in rects {
                let r: Vec<u8> = [*x, *y, *w, *h]
                    .iter()
                    .flat_map(|v| v.to_ne_bytes())
                    .collect();
                sender.send(region, REGION_ADD, &r)?;
            }
            sender.send(surface, SURFACE_SET_INPUT_REGION, &u32s(&[region]))?;
            sender.send(region, REGION_DESTROY, &[])?;
        }
        sender.send(surface, SURFACE_COMMIT, &[]) // the first commit asks for a configure
    })();
    (surface, layer, viewport, sent)
}

fn destroy_extra(sender: &Sender, x: &Extra) {
    let _ = sender.send(x.layer, LS_DESTROY, &[]);
    if x.viewport != 0 {
        let _ = sender.send(x.viewport, VIEWPORT_DESTROY, &[]);
    }
    let _ = sender.send(x.surface, SURFACE_DESTROY, &[]);
    if x.role == Role::Glow {
        if let Some((b, _, _)) = x.buffer {
            let _ = sender.send(b, BUFFER_DESTROY, &[]);
        }
    }
}

/// Takes the probe's surfaces away (what it found stays in [`State::probe`]).
fn remove_probe(sender: &Sender, st: &mut State) {
    st.extras.retain(|x| {
        if x.role != Role::Probe {
            return true;
        }
        destroy_extra(sender, x);
        false
    });
    if let Some(p) = st.probe.as_mut() {
        if p.buffer != 0 {
            let _ = sender.send(p.buffer, BUFFER_DESTROY, &[]);
            p.buffer = 0;
        }
    }
}

fn remove_glow(sender: &Sender, st: &mut State) {
    let mut n = 0;
    st.extras.retain(|x| {
        if x.role != Role::Glow {
            return true;
        }
        destroy_extra(sender, x);
        n += 1;
        false
    });
    st.counters.glows_removed += n;
    st.glow.clear();
}

/// Attaches `buffer` (`bw`×`bh`) to `surface` (`w`×`h` on screen), damaged whole, and commits.
fn present(
    sender: &Sender,
    st: &State,
    surface: u32,
    buffer: u32,
    (bw, bh): (u32, u32),
    (w, h): (u32, u32),
) -> std::io::Result<()> {
    let mut p = u32s(&[buffer]);
    p.extend(0i32.to_ne_bytes());
    p.extend(0i32.to_ne_bytes());
    sender.send(surface, SURFACE_ATTACH, &p)?;
    let (op, dw, dh) = if st.ids.compositor_version >= 4 {
        (SURFACE_DAMAGE_BUFFER, bw, bh)
    } else {
        (SURFACE_DAMAGE, w, h)
    };
    let d: Vec<u8> = [0i32, 0, dw as i32, dh as i32]
        .iter()
        .flat_map(|v| v.to_ne_bytes())
        .collect();
    sender.send(surface, op, &d)?;
    sender.send(surface, SURFACE_COMMIT, &[])
}

/// A probe or glow surface was configured `w`×`h`: what it shows goes up.
fn configure_extra(
    sender: &Sender,
    st: &mut State,
    i: usize,
    serial: u32,
    (w, h): (u32, u32),
) -> std::io::Result<()> {
    if w > 16384 || h > 16384 {
        return Err(wayland::bad("absurd surface size"));
    }
    let (role, side, surface, layer, viewport, current) = {
        let x = &st.extras[i];
        (x.role, x.side, x.surface, x.layer, x.viewport, x.buffer)
    };
    sender.send(layer, LS_ACK_CONFIGURE, &u32s(&[serial]))?;
    if current.is_some_and(|(_, bw, bh)| (bw, bh) == (w, h)) {
        return sender.send(surface, SURFACE_COMMIT, &[]);
    }
    let (buffer, size) = match role {
        Role::Probe | Role::Pickup => {
            let shared = if role == Role::Probe {
                st.probe.as_ref().map(|p| p.buffer)
            } else {
                st.pickup.as_ref().map(|p| p.buffer)
            };
            let Some(b) = shared.filter(|b| *b != 0) else {
                return Ok(()); // already over
            };
            // 1×1 transparent, stretched over the whole output.
            let d: Vec<u8> = [w as i32, h as i32]
                .iter()
                .flat_map(|v| v.to_ne_bytes())
                .collect();
            sender.send(viewport, VIEWPORT_SET_DESTINATION, &d)?;
            (b, (1, 1))
        }
        Role::Glow => {
            let px = glow_pixels(side, w, h);
            let b = shm_buffer(sender, st, w, h, Some(&px))?;
            if let Some((old, _, _)) = current {
                let _ = sender.send(old, BUFFER_DESTROY, &[]);
            }
            (b, (w, h))
        }
    };
    present(sender, st, surface, buffer, size, (w, h))?;
    st.extras[i].buffer = Some((buffer, w, h));
    if current.is_none() {
        match role {
            Role::Probe => {
                st.counters.probe_mapped += 1;
                if let Some(p) = st.probe.as_mut() {
                    p.unmapped = p.unmapped.saturating_sub(1);
                }
            }
            Role::Glow => st.counters.glows += 1,
            Role::Pickup => st.counters.pickup_mapped += 1,
        }
    }
    Ok(())
}

/// Takes the pickup away: its surface, and the drag picked up here if one is going on (the
/// compositor cancels it once its source is gone).
fn remove_pickup(sender: &Sender, st: &mut State) {
    st.extras.retain(|x| {
        if x.role != Role::Pickup {
            return true;
        }
        destroy_extra(sender, x);
        false
    });
    let Some(p) = st.pickup.take() else { return };
    if p.source != 0 {
        let _ = sender.send(p.source, SOURCE_DESTROY, &[]);
    }
    if p.icon != 0 {
        let _ = sender.send(p.icon, SURFACE_DESTROY, &[]);
    }
    for b in [p.buffer, p.icon_buffer] {
        if b != 0 {
            let _ = sender.send(b, BUFFER_DESTROY, &[]);
        }
    }
}

/// The arriving mouse's left press (`serial`) on the pickup: the drag of the received files
/// starts there, copy only, with the Iris icon; then the pickup takes no input any more, so
/// the drag goes to whatever is under the pointer.
fn start_pickup_drag(sender: &Sender, st: &mut State, serial: u32) -> std::io::Result<()> {
    let ids = st.ids;
    let origin = st
        .extras
        .iter()
        .find(|x| x.role == Role::Pickup)
        .map(|x| x.surface)
        .ok_or_else(|| wayland::bad("no pickup surface"))?;
    let count = st.pickup.as_ref().map(|p| p.count).unwrap_or(1);
    let extra: Vec<String> = st
        .pickup
        .as_ref()
        .map(|p| p.extra.offers.iter().map(|(m, _)| m.clone()).collect())
        .unwrap_or_default();
    let source = st.new_id();
    sender.send(ids.ddm, DDM_CREATE_DATA_SOURCE, &u32s(&[source]))?;
    sender.send(source, SOURCE_OFFER, &string(URI_LIST))?;
    for mime in &extra {
        sender.send(source, SOURCE_OFFER, &string(mime))?;
    }
    sender.send(source, SOURCE_SET_ACTIONS, &u32s(&[DND_COPY]))?;
    let icon = st.new_id();
    sender.send(ids.compositor, COMPOSITOR_CREATE_SURFACE, &u32s(&[icon]))?;
    sender.send(
        ids.device,
        DD_START_DRAG,
        &u32s(&[source, origin, icon, serial]),
    )?;
    if let Some(p) = st.pickup.as_mut() {
        p.source = source;
        p.icon = icon;
        p.press = None;
    }
    let (w, h, px) = pickup_icon(count);
    let buffer = shm_buffer(sender, st, w, h, Some(&px))?;
    if let Some(p) = st.pickup.as_mut() {
        p.icon_buffer = buffer;
    }
    present(sender, st, icon, buffer, (w, h), (w, h))?;
    // No input any more: the drag goes on to whatever is under the pointer.
    let region = st.new_id();
    sender.send(ids.compositor, COMPOSITOR_CREATE_REGION, &u32s(&[region]))?;
    for x in st.extras.iter().filter(|x| x.role == Role::Pickup) {
        sender.send(x.surface, SURFACE_SET_INPUT_REGION, &u32s(&[region]))?;
        sender.send(x.surface, SURFACE_COMMIT, &[])?;
    }
    sender.send(region, REGION_DESTROY, &[])?;
    st.counters.pickup_started += 1;
    Ok(())
}

/// The pickup connection's pointer: where it is, and the press that may pick a drag up.
fn pickup_pointer_event(
    sender: &Arc<Sender>,
    st: &Shared,
    s: &mut std::sync::MutexGuard<'_, State>,
    hooks: &Hooks,
    m: &Message,
) -> std::io::Result<()> {
    let r = |off| wayland::read_u32(&m.body, off);
    match m.opcode {
        EV_PTR_ENTER => {
            let (serial, surface) = (r(0)?, r(4)?);
            s.pointer_focus = surface;
            log::debug!("Flow: pickup: the pointer is over the pickup");
            if s.ids.shape_device != 0 {
                // An ordinary arrow while the pointer waits over the pickup.
                let shape = u32s(&[serial, SHAPE_DEFAULT]);
                sender.send(s.ids.shape_device, SHAPE_SET_SHAPE, &shape)?;
            }
        }
        EV_PTR_LEAVE => s.pointer_focus = 0,
        EV_PTR_BUTTON => {
            let (serial, button, state) = (r(0)?, r(8)?, r(12)?);
            let focus = s.pointer_focus;
            let on_pickup = s
                .extras
                .iter()
                .any(|x| x.role == Role::Pickup && x.surface == focus);
            let Some(p) = s.pickup.as_mut() else {
                return Ok(());
            };
            if button != BTN_LEFT || !on_pickup || p.source != 0 {
                log::debug!("Flow: pickup: button {button:#x} state {state} ignored");
            } else if state == 0 {
                p.press = None;
                log::debug!("Flow: pickup: left button released");
            } else {
                // A press may come before our mouse's arrival is noticed (behind a
                // receiver, or a slower Bluetooth stack): it counts if the mouse itself
                // says its left button is down (checked on its evdev node), so a touchpad
                // press is never taken for it.
                if !p.armed {
                    log::info!("Flow: pickup: a press before our mouse's arrival was noticed");
                }
                p.press = Some(serial);
                s.counters.pickup_presses += 1;
                log::info!("Flow: pickup: left press; is it our mouse's?");
                if let Some(h) = hooks.pickup.clone() {
                    let (st, sender) = (st.clone(), sender.clone());
                    let _ = std::thread::Builder::new()
                        .name("flow-pickup-press".into())
                        .spawn(move || confirm_press(st, sender, serial, h));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Starts the drag for the press `serial` once the mouse confirms its left button is down
/// (it may take a moment for its new evdev node to be readable), while that press lasts.
fn confirm_press(st: Shared, sender: Arc<Sender>, serial: u32, hooks: PickupHooks) {
    let end = Instant::now() + CONFIRM_WAIT;
    loop {
        let ours = (hooks.confirm)();
        {
            let mut s = lock(&st.0);
            let pending = s
                .pickup
                .as_ref()
                .is_some_and(|p| p.press == Some(serial) && p.source == 0);
            if !pending {
                log::debug!("Flow: pickup: the press ended before it was confirmed");
                return;
            }
            if ours {
                let started = start_pickup_drag(&sender, &mut s, serial);
                drop(s);
                match started {
                    Ok(()) => {
                        log::info!("Flow: pickup: our mouse's left button is down: drag started");
                        (hooks.news)(PickupNews::Started);
                    }
                    Err(e) => log::info!("Flow: pickup: the drag couldn't start: {e}"),
                }
                return;
            }
        }
        if Instant::now() >= end {
            log::info!(
                "Flow: pickup: the press wasn't our mouse's left button (not held on its evdev node): ignored"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// Events of the drag picked up here: hand the list to a drop target; how it ended.
fn pickup_source_event(
    sender: &Arc<Sender>,
    st: &Shared,
    s: &mut std::sync::MutexGuard<'_, State>,
    fds: &mut std::collections::VecDeque<OwnedFd>,
    m: &Message,
) -> std::io::Result<Option<PickupNews>> {
    let source = s.pickup.as_ref().map(|p| p.source).unwrap_or(0);
    match m.opcode {
        EV_SOURCE_SEND => {
            let (mime, _) = wayland::read_string(&m.body, 0)?;
            let Some(fd) = fds.pop_front() else {
                return Ok(None);
            };
            if mime == URI_LIST {
                let st = st.clone();
                let _ = std::thread::Builder::new()
                    .name("flow-pickup-send".into())
                    .spawn(move || serve_uris(st, fd, source));
            } else if let Some((extra, data)) = s.pickup.as_ref().and_then(|p| {
                let data = p.extra.offers.iter().find(|(m, _)| *m == mime)?.1.clone();
                Some((p.extra.clone(), data))
            }) {
                // Fixed and small: written at once, on a thread of its own all the same
                // (whatever the target's end of the pipe does, the drop never waits).
                if let Some(f) = &extra.on_send {
                    f(&mime);
                }
                s.counters.pickup_extra_sent += 1;
                log::debug!("Flow: pickup: the drop target takes {mime}");
                let _ = std::thread::Builder::new()
                    .name("flow-pickup-send".into())
                    .spawn(move || {
                        use std::io::Write;
                        let _ = std::fs::File::from(fd).write_all(&data);
                    });
            }
        }
        EV_SOURCE_DROP_PERFORMED => {
            s.counters.pickup_drops += 1;
            log::info!("Flow: pickup: dropped");
        }
        EV_SOURCE_FINISHED | EV_SOURCE_CANCELLED => {
            let taken =
                m.opcode == EV_SOURCE_FINISHED && s.pickup.as_ref().is_some_and(|p| p.delivered);
            log::info!(
                "Flow: pickup: the drag {}",
                if taken {
                    "ended: the files were dropped here"
                } else if m.opcode == EV_SOURCE_FINISHED {
                    "ended without the files (they weren't all here in time)"
                } else {
                    "was cancelled"
                }
            );
            remove_pickup(sender, s);
            return Ok(Some(PickupNews::Ended { taken }));
        }
        EV_SOURCE_TARGET => {
            let what = if wayland::read_u32(&m.body, 0)? == 0 {
                "nothing".to_string()
            } else {
                wayland::read_string(&m.body, 0)?.0
            };
            log::debug!("Flow: pickup: the target under the pointer takes {what}");
        }
        EV_SOURCE_ACTION => {
            log::debug!("Flow: pickup: action {}", wayland::read_u32(&m.body, 0)?);
        }
        _ => {}
    }
    Ok(None)
}

/// A drop target asked for the list of the drag `source`: written once every file is here
/// (at most [`SEND_WAIT`] from now); otherwise nothing is written, and it isn't taken.
fn serve_uris(st: Shared, fd: OwnedFd, source: u32) {
    use std::io::Write;
    let (m, cv) = &*st;
    let end = Instant::now() + SEND_WAIT;
    let mut s = lock(m);
    let uris = loop {
        let p = s.pickup.as_ref().filter(|p| p.source == source);
        if p.is_none_or(|p| p.uris.is_some()) {
            break p.and_then(|p| p.uris.clone());
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break None;
        }
        s = cv
            .wait_timeout(s, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    };
    drop(s);
    let Some(uris) = uris else {
        log::info!("Flow: pickup: dropped before every file was here: nothing handed over");
        return; // the fd closes: an empty list
    };
    let mut f = std::fs::File::from(fd);
    if f.write_all(&uris).is_ok() {
        // Marked before the target can see the end of the list (and finish).
        let mut s = lock(m);
        if let Some(p) = s.pickup.as_mut().filter(|p| p.source == source) {
            p.delivered = true;
        }
        s.counters.pickup_sent += 1;
    }
    drop(f);
}

/// The icon of a drag picked up here, `w`×`h` premultiplied ARGB8888: a document in Iris
/// with a lighter folded corner; a second one behind it for several items.
pub fn pickup_icon(count: usize) -> (u32, u32, Vec<u8>) {
    const W: u32 = 40;
    const H: u32 = 44;
    const IRIS: [f32; 3] = [122.0, 102.0, 255.0];
    const LIGHT: [f32; 3] = [200.0, 192.0, 255.0];
    // A 24×30 page with its top-left corner at (x0, y0): (inside, on the fold).
    let page = |px: f32, py: f32, x0: f32, y0: f32| -> (bool, bool) {
        let (w, h, fold) = (24.0, 30.0, 8.0);
        if px < x0 || px >= x0 + w || py < y0 || py >= y0 + h {
            return (false, false);
        }
        let (dx, dy) = (px - (x0 + w - fold), py - y0);
        if dx > 0.0 && dy < fold {
            return if dx > dy {
                (false, false)
            } else {
                (true, true)
            };
        }
        (true, false)
    };
    let mut out = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let (mut front, mut fold, mut back) = (0.0f32, 0.0f32, 0.0f32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                    match page(px, py, 6.0, 10.0) {
                        (true, true) => fold += 1.0,
                        (true, false) => front += 1.0,
                        _ if count > 1 && page(px, py, 12.0, 4.0).0 => back += 1.0,
                        _ => {}
                    }
                }
            }
            let layers = [
                (IRIS, 0.95 * front / 16.0),
                (LIGHT, 0.95 * fold / 16.0),
                (IRIS, 0.45 * back / 16.0),
            ];
            let a: f32 = layers.iter().map(|l| l.1).sum::<f32>().min(1.0);
            let ch = |i: usize| {
                layers
                    .iter()
                    .map(|(c, k)| c[i] * k)
                    .sum::<f32>()
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            let o = ((y * W + x) * 4) as usize;
            out[o] = ch(2); // B
            out[o + 1] = ch(1); // G
            out[o + 2] = ch(0); // R
            out[o + 3] = (a * 255.0).round() as u8; // A
        }
    }
    (W, H, out)
}

fn destroy_surface(sender: &Sender, st: &mut State) {
    let Some(s) = st.surface.take() else { return };
    let _ = sender.send(s.layer, LS_DESTROY, &[]);
    let _ = sender.send(s.surface, SURFACE_DESTROY, &[]);
    for (buffer, _, _) in [s.buffer, s.glow].into_iter().flatten() {
        let _ = sender.send(buffer, BUFFER_DESTROY, &[]);
    }
    st.counters.removed += 1;
}

/// A fully transparent `w`×`h` ARGB buffer: a zero-filled memfd.
fn transparent_buffer(sender: &Sender, st: &mut State, w: u32, h: u32) -> std::io::Result<u32> {
    shm_buffer(sender, st, w, h, None)
}

/// A `w`×`h` ARGB8888 buffer with `pixels` (premultiplied), or zeros (transparent).
fn shm_buffer(
    sender: &Sender,
    st: &mut State,
    w: u32,
    h: u32,
    pixels: Option<&[u8]>,
) -> std::io::Result<u32> {
    let stride = w * 4;
    let size = stride as u64 * h as u64;
    let fd: OwnedFd = rustix::fs::memfd_create(NAMESPACE, rustix::fs::MemfdFlags::CLOEXEC)?;
    rustix::fs::ftruncate(&fd, size)?; // zeros: alpha 0
    if let Some(px) = pixels.filter(|p| p.len() as u64 == size) {
        // Positioned write: leaves the file offset at 0 for whoever reads it.
        let f = std::fs::File::from(fd.try_clone()?);
        std::os::unix::fs::FileExt::write_all_at(&f, px, 0)?;
    }
    let (pool, buffer) = (st.new_id(), st.new_id());
    let mut p = u32s(&[pool]);
    p.extend((size as i32).to_ne_bytes());
    sender.send_fd(st.ids.shm, SHM_CREATE_POOL, &p, fd.as_fd())?;
    let mut b = u32s(&[buffer]);
    for v in [0i32, w as i32, h as i32, stride as i32] {
        b.extend(v.to_ne_bytes());
    }
    b.extend(u32s(&[FORMAT_ARGB8888]));
    sender.send(pool, POOL_CREATE_BUFFER, &b)?;
    sender.send(pool, POOL_DESTROY, &[])?;
    Ok(buffer)
}

/// Shows the glow (files dragged onto the catcher) or the transparent buffer again.
fn light(sender: &Sender, st: &mut State, on: bool) -> std::io::Result<()> {
    let Some((side, surface, idle, glow, lit)) = st
        .surface
        .as_ref()
        .map(|x| (x.side, x.surface, x.buffer, x.glow, x.lit))
    else {
        return Ok(());
    };
    let Some((idle_id, w, h)) = idle else {
        return Ok(()); // not configured yet
    };
    if lit == on {
        return Ok(());
    }
    let buffer = if on {
        match glow.filter(|&(_, gw, gh)| (gw, gh) == (w, h)) {
            Some((id, _, _)) => id,
            None => {
                let px = glow_pixels(side, w, h);
                let id = shm_buffer(sender, st, w, h, Some(&px))?;
                if let Some((old, _, _)) = glow {
                    let _ = sender.send(old, BUFFER_DESTROY, &[]);
                }
                if let Some(x) = st.surface.as_mut() {
                    x.glow = Some((id, w, h));
                }
                id
            }
        }
    } else {
        idle_id
    };
    present(sender, st, surface, buffer, (w, h), (w, h))?;
    if let Some(x) = st.surface.as_mut() {
        x.lit = on;
    }
    if on {
        st.counters.lit += 1;
    }
    Ok(())
}

/// What the connection's thread reports to.
struct Hooks {
    on_drag: OnDrag,
    pickup: Option<PickupHooks>,
}

fn run(
    mut wire: Wire,
    sender: Arc<Sender>,
    st: Shared,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<String>>,
    hooks: Hooks,
) {
    let result = (|| -> std::io::Result<()> {
        while !stop.load(Ordering::SeqCst) {
            for m in wire.read(Duration::from_millis(100))? {
                dispatch(&sender, &st, &hooks, &mut wire.fds, m)?;
            }
            let mut s = lock(&st.0);
            let lingered = s
                .surface
                .as_ref()
                .and_then(|x| x.linger_until)
                .is_some_and(|t| Instant::now() > t);
            if lingered {
                if let Some(d) = s.dnd.take() {
                    let _ = sender.send(d.offer, OFFER_DESTROY, &[]);
                }
                destroy_surface(&sender, &mut s);
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        if !stop.load(Ordering::SeqCst) {
            log::info!("Flow: edge catcher stopped: {e}");
            *lock(&error) = e.to_string();
        }
    }
    let (m, cv) = &*st;
    lock(m).alive = false;
    cv.notify_all();
}

fn dispatch(
    sender: &Arc<Sender>,
    st: &Shared,
    hooks: &Hooks,
    fds: &mut std::collections::VecDeque<OwnedFd>,
    m: Message,
) -> std::io::Result<()> {
    let on_drag = &hooks.on_drag;
    let (lockm, cv) = &**st;
    let mut s = lock(lockm);
    let r = |off| wayland::read_u32(&m.body, off);
    let source = s.pickup.as_ref().map(|p| p.source).unwrap_or(0);
    if source != 0 && m.object == source {
        let news = pickup_source_event(sender, st, &mut s, fds, &m)?;
        drop(s);
        cv.notify_all();
        if let (Some(n), Some(h)) = (news, &hooks.pickup) {
            (h.news)(n);
        }
        return Ok(());
    }
    if s.ids.pointer != 0 && m.object == s.ids.pointer {
        return pickup_pointer_event(sender, st, &mut s, hooks, &m);
    }
    if m.object == s.ids.device {
        match m.opcode {
            EV_DD_DATA_OFFER => {
                let id = r(0)?;
                if s.offers.len() < 64 {
                    s.offers.insert(id, Vec::new());
                } else {
                    let _ = sender.send(id, OFFER_DESTROY, &[]);
                }
            }
            EV_DD_ENTER => {
                let (serial, surface, offer) = (r(0)?, r(4)?, r(16)?);
                if let Some(old) = s.dnd.take() {
                    let _ = sender.send(old.offer, OFFER_DESTROY, &[]);
                }
                s.elsewhere = false;
                if offer == 0 {
                    return Ok(());
                }
                if !s.surface.as_ref().is_some_and(|x| x.surface == surface) {
                    // A probe (only the MIME types matter: never accepted, nothing received),
                    // or a surface of ours already gone.
                    let files = s
                        .offers
                        .remove(&offer)
                        .is_some_and(|mimes| mimes.iter().any(|x| x == URI_LIST));
                    let _ = sender.send(offer, OFFER_DESTROY, &[]);
                    s.elsewhere = true;
                    let probe = s
                        .extras
                        .iter()
                        .any(|x| x.role == Role::Probe && x.surface == surface);
                    if probe {
                        if let Some(p) = s.probe.as_mut() {
                            p.found.get_or_insert(files);
                        }
                        if files {
                            s.counters.probe_files += 1;
                        } else {
                            s.counters.probe_other += 1;
                        }
                        // Gone at once: the drag moves on at its next motion.
                        remove_probe(sender, &mut s);
                        cv.notify_all();
                    }
                    return Ok(());
                }
                let ours = s.surface.as_ref().is_some_and(|x| x.linger_until.is_none());
                let files = ours
                    && s.offers
                        .get(&offer)
                        .is_some_and(|mimes| mimes.iter().any(|x| x == URI_LIST));
                // Nothing accepted yet: the engine decides (Catcher::accept).
                let mut p = u32s(&[serial]);
                p.extend(u32s(&[0]));
                sender.send(offer, OFFER_ACCEPT, &p)?;
                s.dnd = Some(Dnd {
                    offer,
                    serial,
                    files,
                    accepted: false,
                });
                if !files {
                    return Ok(());
                }
                s.counters.entered += 1;
                s.enter_seq += 1;
                light(sender, &mut s, true)?;
                sender.send(offer, OFFER_SET_ACTIONS, &u32s(&[DND_COPY, DND_COPY]))?;
                let (rd, wr) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
                sender.send_fd(offer, OFFER_RECEIVE, &string(URI_LIST), wr.as_fd())?;
                drop(wr);
                let side = s.surface.as_ref().map(|x| x.side).unwrap_or("right");
                let seq = s.enter_seq;
                let st2 = st.clone();
                let on_drag = on_drag.clone();
                let _ = std::thread::Builder::new()
                    .name("flow-catcher-read".into())
                    .spawn(move || read_uri_list(rd, seq, side, st2, on_drag));
            }
            EV_DD_LEAVE => {
                if std::mem::take(&mut s.elsewhere) {
                    return Ok(());
                }
                if let Some(d) = s.dnd.take() {
                    let _ = sender.send(d.offer, OFFER_DESTROY, &[]);
                    s.offers.remove(&d.offer);
                }
                light(sender, &mut s, false)?;
                if s.surface.as_ref().is_some_and(|x| x.linger_until.is_some()) {
                    destroy_surface(sender, &mut s);
                }
            }
            EV_DD_DROP => {
                if std::mem::take(&mut s.elsewhere) {
                    // Let go on a probe in that very instant: nothing was accepted, so the
                    // compositor cancels the drag (the source keeps its files).
                    s.counters.probe_drops += 1;
                    return Ok(());
                }
                if let Some(d) = s.dnd.take() {
                    s.counters.dropped += 1;
                    if d.files && d.accepted {
                        sender.send(d.offer, OFFER_FINISH, &[])?;
                        s.counters.finished += 1;
                    }
                    let _ = sender.send(d.offer, OFFER_DESTROY, &[]);
                    s.offers.remove(&d.offer);
                }
                if s.surface.as_ref().is_some_and(|x| x.linger_until.is_some()) {
                    destroy_surface(sender, &mut s);
                }
            }
            EV_DD_SELECTION => {
                // Never focused, so normally never sent; the clipboard isn't ours to read.
                let id = r(0)?;
                if id != 0 && s.dnd.as_ref().is_none_or(|d| d.offer != id) {
                    s.offers.remove(&id);
                    let _ = sender.send(id, OFFER_DESTROY, &[]);
                }
            }
            _ => {}
        }
    } else if let Some(mimes) = s.offers.get_mut(&m.object) {
        if m.opcode == EV_OFFER_OFFER {
            let (mime, _) = wayland::read_string(&m.body, 0)?;
            if mimes.len() < 256 && mime.len() <= 255 {
                mimes.push(mime);
            }
        }
    } else if s.surface.as_ref().is_some_and(|x| x.layer == m.object) {
        match m.opcode {
            EV_LS_CONFIGURE => {
                let (serial, w, h) = (r(0)?, r(4)?.max(1), r(8)?.max(1));
                let surface = s.surface.as_ref().map(|x| x.surface).unwrap_or(0);
                let layer = m.object;
                sender.send(layer, LS_ACK_CONFIGURE, &u32s(&[serial]))?;
                let current = s.surface.as_ref().and_then(|x| x.buffer);
                if current.is_some_and(|(_, bw, bh)| (bw, bh) == (w, h)) {
                    sender.send(surface, SURFACE_COMMIT, &[])?;
                } else {
                    if w > 16384 || h > 16384 {
                        return Err(wayland::bad("absurd catcher size"));
                    }
                    let buffer = transparent_buffer(sender, &mut s, w, h)?;
                    if let Some((old, _, _)) = current {
                        let _ = sender.send(old, BUFFER_DESTROY, &[]);
                    }
                    present(sender, &s, surface, buffer, (w, h), (w, h))?;
                    if let Some(x) = s.surface.as_mut() {
                        if x.buffer.is_none() {
                            s.counters.mapped += 1;
                        }
                    }
                    let relight = s.surface.as_ref().is_some_and(|x| x.lit);
                    if let Some(x) = s.surface.as_mut() {
                        x.buffer = Some((buffer, w, h));
                        x.lit = false;
                    }
                    if relight {
                        light(sender, &mut s, true)?;
                    }
                }
            }
            EV_LS_CLOSED => {
                if let Some(d) = s.dnd.take() {
                    let _ = sender.send(d.offer, OFFER_DESTROY, &[]);
                }
                destroy_surface(sender, &mut s);
            }
            _ => {}
        }
    } else if let Some(i) = s.extras.iter().position(|x| x.layer == m.object) {
        match m.opcode {
            EV_LS_CONFIGURE => {
                let size = (r(4)?.max(1), r(8)?.max(1));
                configure_extra(sender, &mut s, i, r(0)?, size)?;
            }
            EV_LS_CLOSED => {
                let x = s.extras.remove(i);
                destroy_extra(sender, &x);
                if x.role == Role::Probe && x.buffer.is_none() {
                    if let Some(p) = s.probe.as_mut() {
                        p.unmapped = p.unmapped.saturating_sub(1);
                    }
                }
            }
            _ => {}
        }
        cv.notify_all();
    } else if let Some(o) = s
        .outputs
        .iter_mut()
        .find(|o| o.xdg != 0 && o.xdg == m.object)
    {
        match m.opcode {
            EV_XDG_POSITION => {
                o.pos = (r(0)? as i32 as i64, r(4)? as i32 as i64);
            }
            EV_XDG_SIZE => {
                o.size = (r(0)? as i32 as i64, r(4)? as i32 as i64);
            }
            _ => {}
        }
    } else if m.object == REGISTRY {
        match m.opcode {
            EV_GLOBAL => {
                let name = r(0)?;
                let (iface, off) = wayland::read_string(&m.body, 4)?;
                let version = wayland::read_u32(&m.body, off)?;
                if iface == "wl_output" && s.outputs.len() < 16 {
                    let o = add_output(&mut s, name, version);
                    let mut p = u32s(&[o.global]);
                    p.extend(string("wl_output"));
                    p.extend(u32s(&[1, o.id]));
                    sender.send(REGISTRY, 0, &p)?; // bind
                    if o.xdg != 0 {
                        sender.send(s.ids.xdg_manager, XDG_GET_OUTPUT, &u32s(&[o.xdg, o.id]))?;
                    }
                    s.outputs.push(o);
                }
            }
            EV_GLOBAL_REMOVE => {
                let name = r(0)?;
                s.outputs.retain(|o| o.global != name);
            }
            _ => {}
        }
    } else if s.callbacks.contains_key(&m.object) && m.opcode == EV_CALLBACK_DONE {
        s.callbacks.insert(m.object, true);
        cv.notify_all();
    } else if m.object == DISPLAY && m.opcode == EV_DISPLAY_ERROR {
        return Err(wayland::bad("Wayland protocol error"));
    }
    Ok(())
}

/// Reads the uri-list of the drag that entered as number `seq` (at most URI_LIST_MAX
/// bytes, within URI_LIST_TIMEOUT: slower is a plain drag) and reports its local paths if
/// that drag is still the one on the catcher.
fn read_uri_list(rd: OwnedFd, seq: u64, side: &'static str, st: Shared, on_drag: OnDrag) {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let end = Instant::now() + URI_LIST_TIMEOUT;
    let mut f = std::fs::File::from(rd);
    let mut data = Vec::new();
    let mut buf = vec![0u8; 1 << 14];
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        let ready = {
            let fd = f.as_fd();
            let mut fds = [PollFd::new(&fd, PollFlags::IN)];
            let ts = Timespec {
                tv_sec: left.as_secs() as _,
                tv_nsec: left.subsec_nanos() as _,
            };
            poll(&mut fds, Some(&ts)).unwrap_or(0) > 0
        };
        if !ready {
            return;
        }
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                if data.len() > URI_LIST_MAX {
                    return;
                }
            }
            Err(_) => return,
        }
    }
    let paths = parse_drag_uris(&data);
    if paths.is_empty() {
        return;
    }
    {
        let mut s = lock(&st.0);
        if s.enter_seq != seq || s.dnd.as_ref().is_none_or(|d| !d.files) {
            return;
        }
        s.counters.reported += 1;
    }
    on_drag(side, paths);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::keystate::tests::msg;
    use std::io::Write;
    use std::sync::mpsc;

    enum Cmd {
        /// A drag enters the catcher: its MIME types, and what it writes for uri-list
        /// (after a delay).
        Enter(Vec<&'static str>, Vec<u8>, Duration),
        Drop,
        Leave,
        /// A drag and drop is going on (offering these types), or not: a probe surface that
        /// goes on screen gets it at the next motion.
        Dragging(Option<Vec<&'static str>>),
        /// The pointer comes over the surface with this namespace.
        PointerEnter(&'static str),
        /// The left button goes down or up.
        Button(bool),
        /// Events of the drag started from the pickup: a drop target asks for this type;
        /// the drop; finished; cancelled.
        SourceSend(&'static str),
        SourceDrop,
        SourceFinished,
        SourceCancelled,
    }

    #[derive(Default)]
    struct Log(
        Mutex<Vec<String>>,
        Mutex<Vec<(u32, u32, Vec<u8>)>>,
        Mutex<Option<Vec<u8>>>,
    );

    impl Log {
        fn push(&self, s: String) {
            self.0.lock().unwrap().push(s);
        }
        fn any(&self, prefix: &str) -> bool {
            self.count(prefix) > 0
        }
        /// The pixels of the buffer of this size.
        fn pixels(&self, w: u32, h: u32) -> Option<Vec<u8>> {
            self.1
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|b| (b.0, b.1) == (w, h))
                .map(|b| b.2.clone())
        }
        /// What a drop target read from the pickup's drag.
        fn read(&self) -> Option<Vec<u8>> {
            self.2.lock().unwrap().clone()
        }
        fn has(&self, s: &str) -> bool {
            self.0.lock().unwrap().iter().any(|x| x == s)
        }
        fn count(&self, prefix: &str) -> usize {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|x| x.starts_with(prefix))
                .count()
        }
    }

    /// Plays KWin for the catcher on the other end of a socketpair: the globals, two
    /// outputs with their logical geometry, layer surfaces and drags entering them.
    struct Fake {
        wire: Wire,
        log: Arc<Log>,
        kinds: HashMap<u32, String>,
        outputs: HashMap<u32, Rect>,
        layers: HashMap<u32, (u32, u32, [u32; 2])>, // layer → (surface, output, size)
        configured: HashMap<u32, bool>,
        device: u32,
        surface: u32,
        offer: u32,
        next: u32,
        with_layer_shell: bool,
        pending: Option<(Vec<u8>, Duration)>,
        /// Surface → layer namespace; viewport → surface; region → rectangles.
        ns: HashMap<u32, String>,
        viewports: HashMap<u32, u32>,
        regions: HashMap<u32, Vec<[i32; 4]>>,
        pools: HashMap<u32, Vec<u8>>,
        attached: std::collections::HashSet<u32>,
        drag: Option<Vec<&'static str>>,
        drag_entered: bool,
        pointer: u32,
        source: u32,
    }

    impl Fake {
        fn send(&self, obj: u32, op: u16, payload: &[u8]) {
            let _ = (&self.wire.sock).write_all(&msg(obj, op, payload));
        }

        fn global(&self, reg: u32, name: u32, iface: &str, v: u32) {
            let mut p = u32s(&[name]);
            p.extend(string(iface));
            p.extend(u32s(&[v]));
            self.send(reg, 0, &p);
        }

        fn cmd(&mut self, c: Cmd) {
            match c {
                Cmd::Enter(mimes, data, delay) => {
                    let offer = self.next;
                    self.next += 1;
                    self.offer = offer;
                    self.kinds.insert(offer, "offer".into());
                    self.send(self.device, EV_DD_DATA_OFFER, &u32s(&[offer]));
                    for m in mimes {
                        self.send(offer, EV_OFFER_OFFER, &string(m));
                    }
                    self.pending = Some((data, delay));
                    self.send(
                        self.device,
                        EV_DD_ENTER,
                        &u32s(&[77, self.surface, 10 << 8, 20 << 8, offer]),
                    );
                }
                Cmd::Drop => self.send(self.device, EV_DD_DROP, &[]),
                Cmd::Leave => self.send(self.device, EV_DD_LEAVE, &[]),
                Cmd::Dragging(d) => {
                    self.drag = d;
                    self.drag_entered = false;
                }
                Cmd::PointerEnter(ns) => {
                    let surface = self.surface_of(ns);
                    self.send(self.pointer, EV_PTR_ENTER, &u32s(&[30, surface, 0, 0]));
                }
                Cmd::Button(down) => self.send(
                    self.pointer,
                    EV_PTR_BUTTON,
                    &u32s(&[31, 0, BTN_LEFT, down as u32]),
                ),
                Cmd::SourceSend(mime) => {
                    let (rd, wr) = rustix::pipe::pipe().unwrap();
                    self.wire
                        .send_fd(self.source, EV_SOURCE_SEND, &string(mime), wr.as_fd())
                        .unwrap();
                    drop(wr);
                    let log = self.log.clone();
                    std::thread::spawn(move || {
                        let mut data = Vec::new();
                        let _ = std::fs::File::from(rd).read_to_end(&mut data);
                        *log.2.lock().unwrap() = Some(data);
                    });
                }
                Cmd::SourceDrop => self.send(self.source, EV_SOURCE_DROP_PERFORMED, &[]),
                Cmd::SourceFinished => self.send(self.source, EV_SOURCE_FINISHED, &[]),
                Cmd::SourceCancelled => self.send(self.source, EV_SOURCE_CANCELLED, &[]),
            }
        }

        fn surface_of(&self, ns: &str) -> u32 {
            self.ns
                .iter()
                .filter(|(_, n)| n.as_str() == ns)
                .map(|(s, _)| *s)
                .max()
                .unwrap_or(0)
        }

        fn request(&mut self, m: Message) {
            let kind = if m.object == DISPLAY {
                "display".to_string()
            } else {
                self.kinds.get(&m.object).cloned().unwrap_or_default()
            };
            let r = |off| wayland::read_u32(&m.body, off).unwrap();
            match (kind.as_str(), m.opcode) {
                ("display", 1) => {
                    let reg = r(0);
                    self.kinds.insert(reg, "registry".into());
                    self.global(reg, 1, "wl_compositor", 6);
                    self.global(reg, 2, "wl_shm", 1);
                    if self.with_layer_shell {
                        self.global(reg, 3, "zwlr_layer_shell_v1", 4);
                    }
                    self.global(reg, 4, "wl_seat", 8);
                    self.global(reg, 5, "wl_data_device_manager", 3);
                    self.global(reg, 6, "wl_output", 4);
                    self.global(reg, 7, "wl_output", 4);
                    self.global(reg, 8, "zxdg_output_manager_v1", 3);
                    self.global(reg, 9, "wp_viewporter", 1);
                    self.global(reg, 10, "wp_cursor_shape_manager_v1", 1);
                }
                ("display", 0) => self.send(r(0), 0, &0u32.to_ne_bytes()),
                ("registry", 0) => {
                    let name = r(0);
                    let (iface, off) = wayland::read_string(&m.body, 4).unwrap();
                    let new = wayland::read_u32(&m.body, off + 4).unwrap();
                    if iface == "wl_output" {
                        let rect = if name == 6 {
                            [0, 0, 1920, 1080]
                        } else {
                            [1920, 0, 2560, 1440]
                        };
                        self.outputs.insert(new, rect);
                    }
                    self.kinds.insert(new, iface);
                }
                ("zxdg_output_manager_v1", 1) => {
                    let (xdg, out) = (r(0), r(4));
                    let [x, y, w, h] = self.outputs[&out];
                    let pair = |a: i64, b: i64| {
                        [(a as i32).to_ne_bytes(), (b as i32).to_ne_bytes()].concat()
                    };
                    self.send(xdg, EV_XDG_POSITION, &pair(x, y));
                    self.send(xdg, EV_XDG_SIZE, &pair(w, h));
                }
                ("wl_data_device_manager", 1) => {
                    self.device = r(0);
                    self.kinds.insert(r(0), "device".into());
                }
                ("wl_data_device_manager", DDM_CREATE_DATA_SOURCE) => {
                    self.source = r(0);
                    self.kinds.insert(r(0), "source".into());
                }
                ("source", SOURCE_OFFER) => {
                    let (mime, _) = wayland::read_string(&m.body, 0).unwrap();
                    self.log.push(format!("source offer {mime}"));
                }
                ("source", SOURCE_SET_ACTIONS) => self.log.push(format!("source actions {}", r(0))),
                ("source", SOURCE_DESTROY) => self.log.push("destroy source".into()),
                ("device", DD_START_DRAG) => {
                    let ns = self.ns.get(&r(4)).cloned().unwrap_or_default();
                    self.log.push(format!(
                        "start drag origin {ns} icon {} serial {}",
                        r(8) != 0,
                        r(12)
                    ));
                }
                ("wl_seat", SEAT_GET_POINTER) => {
                    self.pointer = r(0);
                    self.kinds.insert(r(0), "pointer".into());
                    self.log.push("get pointer".into());
                }
                ("wp_cursor_shape_manager_v1", SHAPE_GET_POINTER) => {
                    self.kinds.insert(r(0), "shape".into());
                    self.log.push(format!("cursor shape device for {}", r(4)));
                }
                ("shape", SHAPE_SET_SHAPE) => self.log.push(format!("shape {} {}", r(0), r(4))),
                ("wp_viewporter", VIEWPORTER_GET_VIEWPORT) => {
                    self.kinds.insert(r(0), "viewport".into());
                    self.viewports.insert(r(0), r(4));
                }
                ("viewport", VIEWPORT_SET_DESTINATION) => {
                    self.log.push(format!("destination {} {}", r(0), r(4)))
                }
                ("viewport", VIEWPORT_DESTROY) => self.log.push("destroy viewport".into()),
                ("wl_compositor", 0) => {
                    self.kinds.insert(r(0), "surface".into());
                }
                ("wl_compositor", COMPOSITOR_CREATE_REGION) => {
                    self.kinds.insert(r(0), "region".into());
                }
                ("region", REGION_ADD) => {
                    let v = [r(0) as i32, r(4) as i32, r(8) as i32, r(12) as i32];
                    self.regions.entry(m.object).or_default().push(v);
                    self.log
                        .push(format!("region {} {} {} {}", v[0], v[1], v[2], v[3]));
                }
                ("surface", SURFACE_SET_INPUT_REGION) => {
                    let rects = self.regions.get(&r(0)).cloned().unwrap_or_default();
                    let ns = self.ns.get(&m.object).cloned().unwrap_or_default();
                    self.log.push(format!("input region {ns} {rects:?}"));
                }
                ("zwlr_layer_shell_v1", 0) => {
                    let (layer, surface, output, which) = (r(0), r(4), r(8), r(12));
                    let (ns, _) = wayland::read_string(&m.body, 16).unwrap();
                    self.kinds.insert(layer, "layer".into());
                    self.layers.insert(layer, (surface, output, [0, 0]));
                    self.ns.insert(surface, ns.clone());
                    if ns == NAMESPACE {
                        self.surface = surface;
                    }
                    let out = self.outputs.get(&output).copied().unwrap_or([0; 4]);
                    self.log
                        .push(format!("layer surface on {out:?} layer {which} {ns}"));
                }
                ("layer", LS_SET_SIZE) => {
                    self.layers.get_mut(&m.object).unwrap().2 = [r(0), r(4)];
                    self.log.push(format!("size {} {}", r(0), r(4)));
                }
                ("layer", LS_SET_ANCHOR) => self.log.push(format!("anchor {}", r(0))),
                ("layer", LS_SET_EXCLUSIVE_ZONE) => {
                    self.log.push(format!("exclusive {}", r(0) as i32))
                }
                ("layer", LS_SET_KEYBOARD_INTERACTIVITY) => {
                    self.log.push(format!("keyboard {}", r(0)))
                }
                ("layer", LS_ACK_CONFIGURE) => self.log.push(format!("ack {}", r(0))),
                ("layer", LS_SET_MARGIN) => self.log.push(format!(
                    "margin {} {} {} {}",
                    r(0) as i32,
                    r(4) as i32,
                    r(8) as i32,
                    r(12) as i32
                )),
                ("layer", LS_DESTROY) => {
                    let surface = self.layers.get(&m.object).map(|l| l.0).unwrap_or(0);
                    let ns = self.ns.get(&surface).cloned().unwrap_or_default();
                    self.log.push("destroy layer".into());
                    self.log.push(format!("destroy layer {ns}"));
                }
                ("surface", SURFACE_COMMIT) => {
                    let layer = self
                        .layers
                        .iter()
                        .find(|(_, v)| v.0 == m.object)
                        .map(|(k, v)| (*k, *v));
                    if let Some((layer, (_, output, [w, h]))) = layer {
                        if !self.configured.get(&layer).copied().unwrap_or(false) {
                            self.configured.insert(layer, true);
                            let o = self.outputs[&output];
                            let w = if w == 0 { o[2] as u32 } else { w };
                            let h = if h == 0 { o[3] as u32 } else { h };
                            self.send(layer, EV_LS_CONFIGURE, &u32s(&[5, w, h]));
                        } else {
                            self.log.push("commit".into());
                            let probe =
                                self.ns.get(&m.object).map(String::as_str) == Some(PROBE_NAMESPACE);
                            if probe && self.attached.contains(&m.object) && !self.drag_entered {
                                if let Some(mimes) = self.drag.clone() {
                                    // the drag's next motion: it enters the probe
                                    self.drag_entered = true;
                                    let offer = self.next;
                                    self.next += 1;
                                    self.kinds.insert(offer, "offer".into());
                                    self.send(self.device, EV_DD_DATA_OFFER, &u32s(&[offer]));
                                    for t in mimes {
                                        self.send(offer, EV_OFFER_OFFER, &string(t));
                                    }
                                    self.send(
                                        self.device,
                                        EV_DD_ENTER,
                                        &u32s(&[88, m.object, 0, 0, offer]),
                                    );
                                }
                            }
                        }
                    }
                }
                ("surface", SURFACE_ATTACH) => {
                    self.attached.insert(m.object);
                    self.log.push("attach".into());
                }
                ("surface", SURFACE_DESTROY) => self.log.push("destroy surface".into()),
                ("wl_shm", SHM_CREATE_POOL) => {
                    let (pool, size) = (r(0), r(4));
                    let fd = self.wire.fds.pop_front().unwrap();
                    let mut data = Vec::new();
                    std::fs::File::from(fd).read_to_end(&mut data).unwrap();
                    assert_eq!(data.len(), size as usize);
                    let glow = data.iter().any(|b| *b != 0);
                    self.log.push(
                        if glow {
                            "pool glow"
                        } else {
                            "pool transparent"
                        }
                        .into(),
                    );
                    self.kinds.insert(pool, "pool".into());
                    self.pools.insert(pool, data);
                }
                ("pool", POOL_CREATE_BUFFER) => {
                    self.kinds.insert(r(0), "buffer".into());
                    let data = self.pools.get(&m.object).cloned().unwrap_or_default();
                    self.log.1.lock().unwrap().push((r(8), r(12), data));
                    self.log.push(format!(
                        "buffer {}x{} stride {} format {}",
                        r(8),
                        r(12),
                        r(16),
                        r(20)
                    ));
                }
                ("buffer", BUFFER_DESTROY) => self.log.push("destroy buffer".into()),
                ("offer", OFFER_ACCEPT) => {
                    let mime = if r(4) == 0 {
                        "null".to_string()
                    } else {
                        wayland::read_string(&m.body, 4).unwrap().0
                    };
                    self.log.push(format!("accept {} {mime}", r(0)));
                }
                ("offer", OFFER_SET_ACTIONS) => self.log.push(format!("actions {} {}", r(0), r(4))),
                ("offer", OFFER_RECEIVE) => {
                    let (mime, _) = wayland::read_string(&m.body, 0).unwrap();
                    let fd = self.wire.fds.pop_front().unwrap();
                    self.log.push(format!("receive {mime}"));
                    let (data, delay) = self.pending.take().unwrap_or_default();
                    std::thread::spawn(move || {
                        std::thread::sleep(delay);
                        let _ = std::fs::File::from(fd).write_all(&data);
                    });
                }
                ("offer", OFFER_FINISH) => self.log.push("finish".into()),
                ("offer", OFFER_DESTROY) => self.log.push("destroy offer".into()),
                _ => {}
            }
        }
    }

    fn fake(sock: UnixStream, with_layer_shell: bool) -> (mpsc::Sender<Cmd>, Arc<Log>) {
        let (tx, rx) = mpsc::channel();
        let log = Arc::new(Log::default());
        let l2 = log.clone();
        std::thread::spawn(move || {
            let mut f = Fake {
                wire: Wire::new(sock),
                log: l2,
                kinds: HashMap::new(),
                outputs: HashMap::new(),
                layers: HashMap::new(),
                configured: HashMap::new(),
                device: 0,
                surface: 0,
                offer: 0,
                next: 0xFF00_0000,
                with_layer_shell,
                pending: None,
                ns: HashMap::new(),
                viewports: HashMap::new(),
                regions: HashMap::new(),
                pools: HashMap::new(),
                attached: Default::default(),
                drag: None,
                drag_entered: false,
                pointer: 0,
                source: 0,
            };
            loop {
                while let Ok(c) = rx.try_recv() {
                    f.cmd(c);
                }
                let Ok(msgs) = f.wire.read(Duration::from_millis(10)) else {
                    return;
                };
                for m in msgs {
                    f.request(m);
                }
            }
        });
        (tx, log)
    }

    fn wait(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..150 {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        f()
    }

    type Reports = Arc<Mutex<Vec<(&'static str, Vec<PathBuf>)>>>;

    fn catcher() -> (Catcher, Reports) {
        let reports: Reports = Arc::default();
        let r2 = reports.clone();
        let c = Catcher::new(Arc::new(move |side, paths| {
            r2.lock().unwrap().push((side, paths));
        }));
        (c, reports)
    }

    #[test]
    fn catches_a_file_drag_and_finishes_it_as_a_copy() {
        let (a, b) = UnixStream::pair().unwrap();
        let (tx, log) = fake(b, true);
        let (c, reports) = catcher();
        assert!(c.start(Some(a)), "{}", c.error.lock().unwrap());
        // on the right edge of the second output
        assert!(c.show("right", [1920, 0, 2560, 1440]));
        assert!(wait(|| c.mapped()));
        assert!(log.has("layer surface on [1920, 0, 2560, 1440] layer 3 glisa-flow-catcher"));
        for s in [
            "size 120 0",
            "anchor 11",
            "exclusive -1",
            "keyboard 0",
            "region 119 0 1 1048576", // only the outer pixel takes drops
            "input region glisa-flow-catcher [[119, 0, 1, 1048576]]",
            "ack 5",
        ] {
            assert!(log.has(s), "{s}: {:?}", log.0.lock().unwrap());
        }
        assert!(log.has("buffer 120x1440 stride 480 format 0"));
        assert!(log.has("pool transparent") && !log.has("pool glow")); // invisible while idle
        assert!(wait(|| log.has("commit")));
        // the same place again: nothing new
        assert!(c.show("right", [1920, 0, 2560, 1440]));
        assert_eq!(c.counters().shown, 1);
        // Dolphin drags two files and a web link onto it
        tx.send(Cmd::Enter(
            vec!["text/uri-list", "text/plain"],
            b"file:///tmp/a%20b.txt\r\nhttps://example.com/\r\nfile:///home/x/../y\r\nfile:///tmp/c\r\n".to_vec(),
            Duration::ZERO,
        ))
        .unwrap();
        assert!(wait(|| !reports.lock().unwrap().is_empty()));
        assert_eq!(
            reports.lock().unwrap()[0],
            (
                "right",
                vec![PathBuf::from("/tmp/a b.txt"), PathBuf::from("/tmp/c")]
            )
        );
        assert!(log.has("accept 77 null")); // not before the engine says so
        assert!(wait(|| log.has("pool glow"))); // the edge glows while files are on it
        assert_eq!(c.counters().lit, 1);
        assert!(log.has("actions 1 1")); // copy only
        assert!(log.has("receive text/uri-list"));
        // the crossing goes ahead; the catcher is asked to go but a drag is on it
        assert!(c.accept(true));
        assert!(log.has("accept 77 text/uri-list") || wait(|| log.has("accept 77 text/uri-list")));
        c.hide();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!log.has("destroy layer"));
        // the switch lets go of the button: the drop lands here and finishes as a copy
        tx.send(Cmd::Drop).unwrap();
        assert!(wait(|| log.has("destroy layer")));
        assert!(log.has("finish") && log.has("destroy offer") && log.has("destroy surface"));
        assert!(log.has("destroy buffer"));
        let n = c.counters();
        assert_eq!((n.entered, n.reported, n.dropped, n.finished), (1, 1, 1, 1));
        assert!(!c.accept(true)); // nothing on it any more
        c.close();
    }

    #[test]
    fn plain_and_slow_drags_are_refused() {
        let (a, b) = UnixStream::pair().unwrap();
        let (tx, log) = fake(b, true);
        let (c, reports) = catcher();
        assert!(c.start(Some(a)));
        assert!(c.show("left", [0, 0, 1920, 1080]));
        assert!(wait(|| c.mapped()));
        assert!(log.has("layer surface on [0, 0, 1920, 1080] layer 3 glisa-flow-catcher"));
        assert!(log.has("anchor 7") && log.has("buffer 120x1080 stride 480 format 0"));
        // text from a browser: not files
        tx.send(Cmd::Enter(vec!["text/plain"], Vec::new(), Duration::ZERO))
            .unwrap();
        assert!(wait(|| log.has("accept 77 null")));
        assert!(!c.accept(true));
        tx.send(Cmd::Drop).unwrap();
        assert!(wait(|| log.has("destroy offer")));
        assert!(!log.has("finish") && !log.has("receive text/uri-list"));
        // a uri-list that takes too long: a plain drag
        tx.send(Cmd::Enter(
            vec!["text/uri-list"],
            b"file:///tmp/tarde\r\n".to_vec(),
            Duration::from_millis(600),
        ))
        .unwrap();
        assert!(wait(|| log.has("receive text/uri-list")));
        std::thread::sleep(Duration::from_millis(900));
        assert!(reports.lock().unwrap().is_empty());
        // files, then the drag leaves before any crossing; then the catcher goes at once
        tx.send(Cmd::Leave).unwrap();
        tx.send(Cmd::Enter(
            vec!["text/uri-list"],
            b"file:///tmp/x\r\n".to_vec(),
            Duration::ZERO,
        ))
        .unwrap();
        assert!(wait(|| reports.lock().unwrap().len() == 1));
        let attached = log.count("attach");
        tx.send(Cmd::Leave).unwrap();
        assert!(wait(|| log.count("destroy offer") >= 3));
        // the glow goes when the files leave the edge
        assert!(wait(|| log.count("attach") > attached));
        c.hide();
        assert!(wait(|| log.has("destroy layer")));
        assert_eq!(c.counters().finished, 0);
        // moved to another edge: a new surface on the right output
        assert!(c.show("top", [1920, 0, 2560, 1440]));
        assert!(wait(|| log.has("buffer 2560x120 stride 10240 format 0")));
        c.close();
        assert!(!c.available());
    }

    #[test]
    fn without_layer_shell() {
        let (a, b) = UnixStream::pair().unwrap();
        let _f = fake(b, false);
        let (c, _) = catcher();
        assert!(!c.start(Some(a)));
        assert!(c.error.lock().unwrap().contains("layer-shell"));
        assert!(!c.show("right", [0, 0, 10, 10]));
    }

    #[test]
    fn placements() {
        assert_eq!(placement("left"), (7, GLOW, 0));
        assert_eq!(placement("right"), (11, GLOW, 0));
        assert_eq!(placement("top"), (13, 0, GLOW));
        assert_eq!(placement("bottom"), (14, 0, GLOW));
        // Only the outermost pixel takes drops.
        assert_eq!(input_strip("left").0, 0);
        assert_eq!(input_strip("right").0, GLOW as i32 - 1);
        assert_eq!(input_strip("bottom").1, GLOW as i32 - 1);
        assert_eq!(input_strip("top").3, 1);
    }

    #[test]
    fn the_glow_is_brightest_at_the_edge_and_fades() {
        let (w, h) = (GLOW, 200);
        let px = glow_pixels("right", w, h);
        let alpha = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
        let mid = h / 2;
        // Brightest on the edge, fading away from it, nothing at the far side.
        assert!(alpha(w - 1, mid) > 200);
        assert!(alpha(w - 1, mid) > alpha(w - 30, mid));
        assert!(alpha(w - 30, mid) > alpha(w - 90, mid));
        assert!(alpha(0, mid) < 3);
        // Fading out towards the ends of the edge.
        assert!(alpha(w - 1, 0) < alpha(w - 1, mid) / 4);
        // Premultiplied: no colour channel above its alpha.
        for c in px.chunks(4) {
            assert!(c[0] <= c[3] && c[1] <= c[3] && c[2] <= c[3]);
        }
        // Mirrored for the left edge.
        let left = glow_pixels("left", w, h);
        assert_eq!(left[((mid * w) * 4 + 3) as usize], alpha(w - 1, mid));
    }

    #[test]
    fn the_probe_finds_a_file_drag_and_goes_at_once() {
        let (a, b) = UnixStream::pair().unwrap();
        let (tx, log) = fake(b, true);
        let (c, reports) = catcher();
        assert!(c.start(Some(a)));
        // Dolphin is dragging files (and text) somewhere
        tx.send(Cmd::Dragging(Some(vec!["text/uri-list", "text/plain"])))
            .unwrap();
        std::thread::sleep(Duration::from_millis(50)); // the fake takes it in
        let t0 = Instant::now();
        assert_eq!(c.probe(Duration::from_millis(150)), Some(true));
        assert!(t0.elapsed() < Duration::from_millis(1000));
        for s in [
            // invisible surfaces over both outputs: a 1×1 transparent buffer, stretched
            "layer surface on [0, 0, 1920, 1080] layer 3 glisa-flow-probe",
            "layer surface on [1920, 0, 2560, 1440] layer 3 glisa-flow-probe",
            "anchor 15",
            "size 0 0",
            "exclusive -1",
            "keyboard 0",
            "buffer 1x1 stride 4 format 0",
            "destination 1920 1080",
            "destination 2560 1440",
        ] {
            assert!(log.has(s), "{s}: {:?}", log.0.lock().unwrap());
        }
        assert!(!log.any("input region glisa-flow-probe")); // the whole surface
        assert!(!log.has("pool glow"));
        // gone as soon as the drag entered; nothing accepted, read or finished
        assert!(wait(|| log.count("destroy layer glisa-flow-probe") == 2));
        assert!(wait(|| log.count("destroy viewport") == 2));
        assert!(wait(|| log.has("destroy offer")));
        assert!(!log.any("accept") && !log.any("receive") && !log.any("actions"));
        assert!(reports.lock().unwrap().is_empty());
        let n = c.counters();
        assert_eq!(
            (n.probes, n.probe_mapped, n.probe_files, n.probe_missed),
            (1, 2, 1, 0)
        );
        // the button let go in that very instant: the drop on the probe is refused
        tx.send(Cmd::Drop).unwrap();
        assert!(wait(|| c.counters().probe_drops == 1));
        assert!(!log.has("finish"));
        c.close();
    }

    #[test]
    fn the_probe_ignores_other_drags_and_finds_nothing_without_one() {
        let (a, b) = UnixStream::pair().unwrap();
        let (tx, log) = fake(b, true);
        let (c, _) = catcher();
        assert!(c.start(Some(a)));
        // a link dragged from a browser: a drag, not files
        tx.send(Cmd::Dragging(Some(vec!["text/plain", "text/x-moz-url"])))
            .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(c.probe(Duration::from_millis(150)), Some(false));
        assert_eq!(c.counters().probe_other, 1);
        // a selection, a window being moved: no drag ever enters
        tx.send(Cmd::Dragging(None)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let t0 = Instant::now();
        assert_eq!(c.probe(Duration::from_millis(150)), None);
        assert!(t0.elapsed() >= Duration::from_millis(150));
        assert!(t0.elapsed() < Duration::from_millis(1000));
        assert!(wait(|| log.count("destroy layer glisa-flow-probe") == 4));
        let n = c.counters();
        assert_eq!((n.probes, n.probe_missed, n.probe_files), (2, 1, 0));
        assert!(!log.any("accept") && !log.any("receive"));
        c.close();
    }

    #[test]
    fn glow_strips_take_no_input_and_go_when_asked() {
        let (a, b) = UnixStream::pair().unwrap();
        let (_tx, log) = fake(b, true);
        let (c, _) = catcher();
        assert!(c.start(Some(a)));
        let edges = [
            GlowEdge {
                side: "right",
                output: [1920, 0, 2560, 1440],
                start: 0,
                len: 1440,
            },
            GlowEdge {
                side: "left",
                output: [0, 0, 1920, 1080],
                start: 100,
                len: 500,
            },
        ];
        assert!(c.glow(&edges));
        assert!(wait(|| c.glowing() == 2));
        for s in [
            "layer surface on [1920, 0, 2560, 1440] layer 3 glisa-flow-glow",
            "layer surface on [0, 0, 1920, 1080] layer 3 glisa-flow-glow",
            "anchor 11", // the whole right side
            "anchor 5",  // a stretch of the left side…
            "size 120 500",
            "margin 100 0 0 0", // …100 px down
            "buffer 120x1440 stride 480 format 0",
            "buffer 120x500 stride 480 format 0",
            "pool glow",
        ] {
            assert!(wait(|| log.has(s)), "{s}: {:?}", log.0.lock().unwrap());
        }
        // visual only: an empty input region
        assert_eq!(log.count("input region glisa-flow-glow []"), 2);
        // exactly the catcher's glow
        assert_eq!(log.pixels(120, 500), Some(glow_pixels("left", 120, 500)));
        assert_eq!(log.pixels(120, 1440), Some(glow_pixels("right", 120, 1440)));
        // asked again: nothing new
        assert!(c.glow(&edges));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(log.count("layer surface on"), 2);
        // the button is let go: gone
        assert!(c.glow(&[]));
        assert!(wait(|| log.count("destroy layer glisa-flow-glow") == 2));
        assert_eq!(c.glowing(), 0);
        let n = c.counters();
        assert_eq!((n.glows, n.glows_removed), (2, 2));
        assert!(log.count("destroy buffer") >= 2);
        c.close();
    }

    type News = Arc<Mutex<Vec<PickupNews>>>;

    fn pickup(sock: UnixStream) -> (Catcher, Arc<AtomicBool>, News) {
        let ours = Arc::new(AtomicBool::new(false));
        let news: News = Arc::default();
        let (o2, n2) = (ours.clone(), news.clone());
        let c = Catcher::for_pickup(PickupHooks {
            confirm: Arc::new(move || o2.load(Ordering::SeqCst)),
            news: Arc::new(move |n| n2.lock().unwrap().push(n)),
        });
        assert!(c.start(Some(sock)));
        (c, ours, news)
    }

    #[test]
    fn the_pickup_starts_the_drag_with_our_mouses_press() {
        let (a, b) = UnixStream::pair().unwrap();
        let (tx, log) = fake(b, true);
        let (c, ours, news) = pickup(a);
        assert!(c.pickup_possible());
        assert!(log.has("get pointer") && log.any("cursor shape device"));
        // files are coming: the pickup waits over the output where the pointer will enter
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let a2 = asked.clone();
        let extra = PickupExtra {
            offers: vec![("application/x-glisa-test".into(), b"alpha".to_vec())],
            on_send: Some(Arc::new(move |m| a2.lock().unwrap().push(m.to_string()))),
        };
        assert!(c.pickup_expect([1920, 0, 2560, 1440], 3, extra));
        assert!(wait(|| c.pickup_waiting()));
        for s in [
            "layer surface on [1920, 0, 2560, 1440] layer 3 glisa-flow-pickup",
            "anchor 15",
            "keyboard 0",
            "destination 2560 1440",
        ] {
            assert!(log.has(s), "{s}: {:?}", log.0.lock().unwrap());
        }
        assert!(!log.any("input region glisa-flow-pickup")); // takes the press anywhere
        // the pointer comes over it: an ordinary arrow
        tx.send(Cmd::PointerEnter(PICKUP_NAMESPACE)).unwrap();
        assert!(wait(|| log.has("shape 30 1")));
        // a press before our mouse arrived, from a touchpad: not our mouse's button, ignored
        tx.send(Cmd::Button(true)).unwrap();
        std::thread::sleep(CONFIRM_WAIT + Duration::from_millis(200));
        assert!(!log.any("start drag"));
        tx.send(Cmd::Button(false)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        c.pickup_arrived();
        // a press while our mouse's left button isn't down: ignored
        tx.send(Cmd::Button(true)).unwrap();
        std::thread::sleep(CONFIRM_WAIT + Duration::from_millis(200));
        assert!(!log.any("start drag"));
        tx.send(Cmd::Button(false)).unwrap();
        // our mouse's press: the drag starts with its serial, copy only, with the icon
        ours.store(true, Ordering::SeqCst);
        tx.send(Cmd::Button(true)).unwrap();
        assert!(
            wait(|| log.any("start drag")),
            "{:?}",
            log.0.lock().unwrap()
        );
        assert!(log.has("start drag origin glisa-flow-pickup icon true serial 31"));
        assert!(log.has("source offer text/uri-list") && log.has("source actions 1"));
        assert!(log.has("source offer application/x-glisa-test"));
        assert!(log.has("buffer 40x44 stride 160 format 0"));
        // from now on it takes no input: the drag goes wherever the user takes it
        assert!(wait(|| log.has("input region glisa-flow-pickup []")));
        assert!(wait(|| *news.lock().unwrap() == [PickupNews::Started]));
        assert!(c.pickup_dragging() && !c.pickup_waiting());
        assert_eq!(c.counters().pickup_presses, 3); // the touchpad's, the unconfirmed one, ours
        // an extra type: written at once, even before the files are here; its hook is told
        tx.send(Cmd::SourceSend("application/x-glisa-test"))
            .unwrap();
        assert!(wait(|| log.read().is_some()));
        assert_eq!(log.read().unwrap(), b"alpha");
        assert_eq!(*asked.lock().unwrap(), ["application/x-glisa-test"]);
        assert_eq!(c.counters().pickup_extra_sent, 1);
        *log.2.lock().unwrap() = None;
        // a type it doesn't offer: nothing (the fd just closes)
        tx.send(Cmd::SourceSend("text/plain")).unwrap();
        assert!(wait(|| log.read().is_some()));
        assert_eq!(log.read().unwrap(), b"");
        *log.2.lock().unwrap() = None;
        // a drop target asks for the list before every file is here: it waits for them
        tx.send(Cmd::SourceSend(URI_LIST)).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(log.read(), None);
        c.pickup_ready(b"file:///tmp/staged/a%20b.txt\r\n".to_vec());
        assert!(wait(|| log.read().is_some()));
        assert_eq!(log.read().unwrap(), b"file:///tmp/staged/a%20b.txt\r\n");
        tx.send(Cmd::SourceDrop).unwrap();
        tx.send(Cmd::SourceFinished).unwrap();
        assert!(wait(|| news.lock().unwrap().len() == 2));
        assert_eq!(news.lock().unwrap()[1], PickupNews::Ended { taken: true });
        assert!(log.has("destroy source") && log.has("destroy layer glisa-flow-pickup"));
        assert!(!c.pickup_dragging() && !c.pickup_waiting());
        let n = c.counters();
        assert_eq!((n.pickup_started, n.pickup_sent, n.pickup_drops), (1, 1, 1));
        c.close();
    }

    #[test]
    fn the_pickup_hands_over_nothing_late_and_never_lingers() {
        let (a, b) = UnixStream::pair().unwrap();
        let (tx, log) = fake(b, true);
        let (c, ours, news) = pickup(a);
        // no press came: taken away, nothing reported (the engine knows)
        assert!(c.pickup_expect([0, 0, 1920, 1080], 1, PickupExtra::default()));
        assert!(wait(|| c.pickup_waiting()));
        c.pickup_cancel();
        assert!(wait(|| log.has("destroy layer glisa-flow-pickup")));
        assert!(!c.pickup_waiting());
        // dropped before the files are all here: after SEND_WAIT, an empty list
        assert!(c.pickup_expect([0, 0, 1920, 1080], 1, PickupExtra::default()));
        assert!(wait(|| c.pickup_waiting()));
        c.pickup_arrived();
        ours.store(true, Ordering::SeqCst);
        tx.send(Cmd::PointerEnter(PICKUP_NAMESPACE)).unwrap();
        tx.send(Cmd::Button(true)).unwrap();
        assert!(wait(|| c.pickup_dragging()));
        // a second pickup can't replace a drag going on
        assert!(!c.pickup_expect([0, 0, 1920, 1080], 1, PickupExtra::default()));
        let t0 = Instant::now();
        tx.send(Cmd::SourceSend(URI_LIST)).unwrap();
        assert!(wait(|| log.read().is_some()));
        assert!(t0.elapsed() >= SEND_WAIT);
        assert_eq!(log.read().unwrap(), b"");
        tx.send(Cmd::SourceFinished).unwrap();
        assert!(wait(|| news.lock().unwrap().len() == 2));
        assert_eq!(news.lock().unwrap()[1], PickupNews::Ended { taken: false });
        // cancelled while dragging (Escape): not taken either
        assert!(c.pickup_expect([0, 0, 1920, 1080], 1, PickupExtra::default()));
        c.pickup_arrived();
        assert!(wait(|| c.pickup_waiting()));
        tx.send(Cmd::Button(false)).unwrap();
        tx.send(Cmd::PointerEnter(PICKUP_NAMESPACE)).unwrap();
        tx.send(Cmd::Button(true)).unwrap();
        assert!(wait(|| c.pickup_dragging()));
        tx.send(Cmd::SourceCancelled).unwrap();
        assert!(wait(|| news.lock().unwrap().len() == 4));
        assert_eq!(news.lock().unwrap()[3], PickupNews::Ended { taken: false });
        c.close();
    }

    #[test]
    fn the_pickup_icon() {
        let (w, h, one) = pickup_icon(1);
        let (_, _, two) = pickup_icon(5);
        assert_eq!((w, h, one.len()), (40, 44, 40 * 44 * 4));
        // premultiplied, Iris, a second page behind for several
        for c in one.chunks(4).chain(two.chunks(4)) {
            assert!(c[0] <= c[3] && c[1] <= c[3] && c[2] <= c[3]);
        }
        let alpha = |px: &[u8], x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
        assert!(alpha(&one, 15, 30) > 200); // inside the front page
        assert_eq!(alpha(&one, 0, 0), 0);
        assert_eq!(alpha(&one, 33, 5), 0); // the folded corner
        assert_eq!(alpha(&one, 33, 20), 0);
        assert!(alpha(&two, 33, 20) > 50); // the page behind shows there
    }
}
