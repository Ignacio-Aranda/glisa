//! The edge engine (docs/FLOW.md §2).
//!
//! Crossing, on the computer that has the device: an `edge` event on a side with a peer,
//! outside the corner dead zones, not during the cooldown, once per visit to the edge, not
//! while a mouse button is held (a drag to the edge is not a switch), with Ctrl held when
//! the user asked for that → `switch {side, t, devices}` to the peer (0.8 s) → release
//! everything held → `set_host` on every target device in parallel → cooldown → send what
//! was copied. On the receiving computer: `switch` stores a pending arrival (15 s) and the
//! cursor is placed at the matching point of the entry edge as soon as the device arrives,
//! unless it became usable too long ago (the user is already moving it).
//!
//! Files dragged to the edge (KDE Wayland): with a button held at an edge that leads to an
//! online peer that takes transfers, the backend puts up the edge catcher; it goes away
//! when the button is let go or the pointer leaves the edge. Files dragged onto it are the
//! one case where the pointer crosses with the button held: `drag` notice (peers that
//! announced `drag`) → `switch` → the catcher accepts the drop as a copy → `set_host` (the
//! compositor lets go of the button, which drops on the catcher) → the files are sent.
//!
//! Before that, as soon as files are picked up anywhere (the backend's drag watch), every
//! outer edge of the desktop on a side whose peer could take them glows, until the button
//! is let go, the files cross, or [`GLOW_TIMEOUT`]. On the receiving computer, a `switch`
//! after a `drag` notice puts up the pickup, and the arrival of the mouse arms it
//! (`pickup.rs`).

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use glisa_common::{AppearingDevice, DeviceKind, FlowDevice};
use glisa_i18n::n_;
use serde_json::{Value, json};

use super::{
    ARRIVAL_WINDOW, AtEdge, Catcher, FileDrag, FlowService, Item, LATE_PLACEMENT, Pending,
    SWITCH_TIMEOUT, SendReason, refused_text,
};
use crate::geometry::{GlowEdge, entry_point, in_corner, outer_edges, relative};
use crate::proto::Msg;
use crate::store::clean_name;
use crate::text::{Text, plain};
use crate::util::{lock, unix_time};
use crate::{CAP_DRAG, CAP_TRANSFER, CLIPBOARD_MAX, POINTER_KINDS, SIDES, opposite, side_of};

/// The `drag` notice must not hold the crossing up for long.
const DRAG_NOTICE_TIMEOUT: Duration = Duration::from_millis(600);
/// How long files dragged onto the edge glow there before the mouse crosses.
pub(crate) const DRAG_DWELL: Duration = Duration::from_millis(300);
/// The edges stop glowing after this even if the end of the drag went unseen.
pub const GLOW_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the mouse buttons are looked at while the catcher is up.
const BUTTONS_EVERY: Duration = Duration::from_millis(40);

/// Devices to switch, with their channels.
type Targets = Vec<(FlowDevice, u8)>;

/// The wire name of a device kind.
pub fn kind_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Mouse => "mouse",
        DeviceKind::Trackball => "trackball",
        DeviceKind::Touchpad => "trackpad",
        DeviceKind::Keyboard => "keyboard",
        DeviceKind::Receiver => "receiver",
        DeviceKind::Other => "unknown",
    }
}

pub fn is_pointer(kind: DeviceKind) -> bool {
    POINTER_KINDS.contains(&kind_name(kind))
}

fn norm(name: &str) -> String {
    name.to_lowercase()
        .split('.')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn entry_index(e: &Value) -> Option<i64> {
    e.get("index").and_then(Value::as_i64)
}

/// The channel whose Easy-Switch name matches the peer's host name (case and punctuation
/// ignored, domain stripped; exact match first, then prefix).
pub fn suggest_channel(dev: &FlowDevice, peer_name: &str) -> Option<u8> {
    let target = norm(peer_name);
    if target.is_empty() {
        return None;
    }
    let entries: Vec<&Value> = dev
        .host_entries
        .iter()
        .filter(|e| {
            e.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| !n.is_empty())
        })
        .filter(|e| {
            entry_index(e) != Some(dev.current_host as i64)
                && entry_index(e).unwrap_or(99) < dev.hosts_count as i64
        })
        .collect();
    let name = |e: &Value| norm(e.get("name").and_then(Value::as_str).unwrap_or(""));
    for e in &entries {
        if name(e) == target {
            return entry_index(e).map(|i| i as u8);
        }
    }
    for e in &entries {
        let n = name(e);
        if !n.is_empty() && (n.starts_with(&target) || target.starts_with(&n)) {
            return entry_index(e).map(|i| i as u8);
        }
    }
    None
}

impl FlowService {
    /// Devices with Easy-Switch channels.
    pub(crate) fn devices(&self) -> Vec<FlowDevice> {
        self.host()
            .map(|h| h.flow_devices())
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.hosts_count > 0)
            .collect()
    }

    /// (channel, chosen by the user).
    pub(crate) fn channel_for(&self, dev: &FlowDevice, pid: &str) -> (Option<u8>, bool) {
        if let Some(ch) = self.store.channel(&dev.key, pid) {
            return (Some(ch), true);
        }
        (
            self.store
                .peer(pid)
                .and_then(|p| suggest_channel(dev, &p.name)),
            false,
        )
    }

    /// (pointer devices, keyboards) to switch to `pid`, with their channels.
    fn targets(&self, pid: &str) -> (Targets, Targets) {
        let keyboard = self.store.options().keyboard;
        let (mut pointers, mut keyboards) = (Vec::new(), Vec::new());
        for d in self.devices() {
            if !d.online {
                continue;
            }
            let pointer = is_pointer(d.kind);
            if !pointer && !(d.kind == DeviceKind::Keyboard && keyboard) {
                continue;
            }
            let (Some(ch), _) = self.channel_for(&d, pid) else {
                continue;
            };
            if ch >= d.hosts_count || ch == d.current_host {
                continue;
            }
            if pointer {
                pointers.push((d, ch))
            } else {
                keyboards.push((d, ch))
            }
        }
        (pointers, keyboards)
    }

    pub(crate) fn worker(&self, rx: mpsc::Receiver<Item>) {
        while !self.stop_flag.load(Ordering::SeqCst) {
            let item = match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(i) => i,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.expire();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            self.timers();
            let run = std::panic::AssertUnwindSafe(|| match item {
                Item::Edge(e) => {
                    lock(&self.st).at_edge = Some(e);
                    self.try_cross();
                }
                Item::Leave(side) => {
                    let catcher = {
                        let mut st = lock(&self.st);
                        if st.at_edge.as_ref().is_some_and(|e| e.side == side) {
                            st.at_edge = None;
                        }
                        st.armed.insert(side, true);
                        st.catcher.as_ref().is_some_and(|c| c.side == side)
                    };
                    if catcher {
                        self.hide_catcher();
                    }
                }
                Item::FileDrag(side, paths) => {
                    let wanted = {
                        let mut st = lock(&self.st);
                        let up = st.catcher.as_ref().is_some_and(|c| c.side == side);
                        if up {
                            st.file_drag = Some(FileDrag { side, paths });
                        }
                        up
                    };
                    // The edge glows for a moment before the mouse goes (and pulling back
                    // from the edge in that time cancels it).
                    if wanted {
                        let me = self.me.clone();
                        let _ = std::thread::Builder::new()
                            .name("flow-drag-dwell".into())
                            .spawn(move || {
                                std::thread::sleep(DRAG_DWELL);
                                if let Some(svc) = me.upgrade() {
                                    svc.post(Item::DragDwell(side));
                                }
                            });
                    }
                }
                Item::DragDwell(side) => {
                    let ready = lock(&self.st)
                        .file_drag
                        .as_ref()
                        .is_some_and(|d| d.side == side);
                    if ready {
                        self.try_cross();
                    }
                }
                Item::Released => self.hide_catcher(),
                Item::FilesDragging(true) => self.show_glow(),
                Item::FilesDragging(false) => self.hide_glow(),
                Item::Pickup(news) => self.pickup_news(news),
                Item::Ctrl(held) => {
                    if held {
                        self.try_cross();
                    }
                }
                Item::Online(id) => self.online_now(&id),
                Item::Appearing => self.appearing_now(),
                Item::Stop => {}
            });
            if std::panic::catch_unwind(run).is_err() {
                log::error!("Flow: engine");
            }
            if self.stop_flag.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    fn expire(&self) {
        {
            let mut st = lock(&self.st);
            if st
                .pending
                .as_ref()
                .is_some_and(|p| Instant::now() > p.until)
            {
                st.pending = None;
            }
        }
        self.timers();
    }

    /// What ends by itself: the glow, the pickup.
    fn timers(&self) {
        self.expire_glow();
        self.expire_pickup();
    }

    fn expire_glow(&self) {
        let over = lock(&self.st)
            .glow_until
            .is_some_and(|t| Instant::now() >= t);
        if over {
            self.hide_glow();
        }
    }

    /// The drag watch runs while files may be dragged across the edge from here.
    pub(crate) fn update_drag_watch(&self) {
        let opts = self.store.options();
        let on = self.running() && opts.drag_files && opts.files;
        if let Some(b) = self.backend() {
            b.drag_watch(on);
        }
        if !on {
            self.hide_glow();
        }
    }

    /// Where files being dragged glow: every outer edge of the desktop on a side whose peer
    /// could take them now (the catcher's conditions: both options on, the peer online
    /// and taking transfers, the mouse here).
    pub(crate) fn glow_edges(&self) -> Vec<GlowEdge> {
        let opts = self.store.options();
        if !self.running() || !opts.drag_files || !opts.files {
            return Vec::new();
        }
        if !self.backend().is_some_and(|b| b.drag_catcher_available()) {
            return Vec::new();
        }
        let screens = self.screens();
        let mut out = Vec::new();
        for side in SIDES {
            let Some(pid) = self.store.peer_at(side) else {
                continue;
            };
            let Some(link) = self.link(&pid) else {
                continue;
            };
            if !link.online() || !link.caps().contains(CAP_TRANSFER) {
                continue;
            }
            if self.targets(&pid).0.is_empty() {
                continue;
            }
            out.extend(outer_edges(&screens, side));
        }
        out
    }

    /// Files were picked up somewhere: the edges they could cross glow.
    fn show_glow(&self) {
        let edges = self.glow_edges();
        if edges.is_empty() {
            self.hide_glow();
            return;
        }
        let timeout = self.hooks().glow_timeout.unwrap_or(GLOW_TIMEOUT);
        lock(&self.st).glow_until = Some(Instant::now() + timeout);
        if let Some(b) = self.backend() {
            b.drag_glow(&edges);
        }
    }

    fn hide_glow(&self) {
        let was = lock(&self.st).glow_until.take().is_some();
        if was {
            if let Some(b) = self.backend() {
                b.drag_glow(&[]);
            }
        }
    }

    pub(crate) fn try_cross(&self) {
        let (pid, e) = {
            let st = lock(&self.st);
            let Some(e) = st.at_edge.clone() else { return };
            if !self.running() {
                return;
            }
            let Some(pid) = self.store.peer_at(e.side) else {
                return;
            };
            if Instant::now() < st.cooldown_until || !st.armed.get(e.side).copied().unwrap_or(true)
            {
                return;
            }
            (pid, e)
        };
        let opts = self.store.options();
        if in_corner(&e.rect, e.side, e.x, e.y, opts.corner_px) {
            return;
        }
        let backend = self.backend();
        if opts.require_ctrl {
            match backend.as_ref().and_then(|b| b.ctrl_held()) {
                None => {
                    self.notice(
                        Text::new(n_!(
                            "Flow is set to switch only while Ctrl is held, but this desktop doesn't report whether it is"
                        )),
                        "warn",
                        Some("ctrl".into()),
                        Duration::from_secs(60),
                    );
                    return;
                }
                Some(false) => return, // waits at the edge: a Ctrl press will retry
                Some(true) => {}
            }
        }
        // A button held: a window or a file is being dragged to the edge. Never a switch
        // (the button would be released on the other computer, dropping it at the edge)…
        // except files dragged onto the edge catcher, which go along.
        let mut dragged: Option<Vec<PathBuf>> = None;
        match backend.as_ref().and_then(|b| b.buttons_held()) {
            Some(true) => {
                let drag = {
                    let mut st = lock(&self.st);
                    match st.file_drag.take() {
                        Some(d) if d.side == e.side => Some(d.paths),
                        _ => None,
                    }
                };
                match drag {
                    Some(paths) => dragged = Some(paths),
                    None => {
                        self.want_catcher(&pid, &e);
                        return;
                    }
                }
            }
            Some(false) => {}
            None => {
                let mut st = lock(&self.st);
                if !st.held_unknown_logged {
                    st.held_unknown_logged = true;
                    log::info!(
                        "Flow: this desktop doesn't say whether a mouse button is held; switching anyway"
                    );
                }
            }
        }
        {
            let mut st = lock(&self.st);
            if st.at_edge.as_ref().map(|a| a.side) != Some(e.side)
                || !st.armed.get(e.side).copied().unwrap_or(true)
            {
                return;
            }
            st.armed.insert(e.side, false); // one attempt per visit to the edge
        }
        self.cross(&pid, e.side, e.x, e.y, e.rect, dragged);
    }

    /// A button is held at the edge `e`, which leads to `pid`: put up the edge catcher if
    /// files could cross there (Flow sends files, the peer is online and takes transfers,
    /// the mouse is here).
    fn want_catcher(&self, pid: &str, e: &AtEdge) {
        let Some(backend) = self.backend() else {
            return;
        };
        let opts = self.store.options();
        if !opts.drag_files || !opts.files || !backend.drag_catcher_available() {
            return;
        }
        let Some(link) = self.link(pid) else { return };
        if !link.online() || !link.caps().contains(CAP_TRANSFER) {
            return;
        }
        if self.targets(pid).0.is_empty() {
            return;
        }
        {
            let mut st = lock(&self.st);
            if st
                .catcher
                .as_ref()
                .is_some_and(|c| c.side == e.side && c.rect == e.rect)
            {
                return;
            }
            st.catcher = Some(Catcher {
                side: e.side,
                rect: e.rect,
            });
            st.file_drag = None;
        }
        backend.drag_catcher(Some((e.side, e.rect)));
        self.watch_buttons();
    }

    fn hide_catcher(&self) {
        let was = {
            let mut st = lock(&self.st);
            st.file_drag = None;
            st.catcher.take().is_some()
        };
        if was {
            if let Some(b) = self.backend() {
                b.drag_catcher(None);
            }
        }
    }

    /// While the catcher is up, look at the mouse buttons: once they are let go (or can't
    /// be read any more: the mouse went to the other computer), it goes away.
    fn watch_buttons(&self) {
        if self.catcher_watch.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.me.clone();
        let spawned = std::thread::Builder::new()
            .name("flow-catcher".into())
            .spawn(move || {
                let mut posted = false;
                loop {
                    std::thread::sleep(BUTTONS_EVERY);
                    let Some(svc) = me.upgrade() else { return };
                    let up = |svc: &FlowService| svc.running() && lock(&svc.st).catcher.is_some();
                    if !up(&svc) {
                        svc.catcher_watch.store(false, Ordering::SeqCst);
                        // A catcher may have gone up between the check and the store.
                        if !up(&svc) || svc.catcher_watch.swap(true, Ordering::SeqCst) {
                            return;
                        }
                        posted = false;
                        continue;
                    }
                    let held = svc.backend().and_then(|b| b.buttons_held());
                    if held == Some(true) {
                        posted = false;
                    } else if !posted {
                        posted = true;
                        svc.post(Item::Released);
                    }
                }
            });
        if spawned.is_err() {
            self.catcher_watch.store(false, Ordering::SeqCst);
        }
    }

    /// Tells the peer files dragged across the edge are coming (it gets ready to let the
    /// drag go on there). Only
    /// to a peer that announced `drag`; never holds the crossing up for long.
    fn drag_notice(&self, pid: &str, paths: &[PathBuf]) {
        let Some(link) = self.link(pid) else { return };
        if !link.caps().contains(CAP_DRAG) {
            return; // an older Glisa there: it gets the files as copied files
        }
        let Some(summary) = crate::drag::summarize(paths, crate::drag::SUMMARY_BUDGET) else {
            return;
        };
        if link
            .request("drag", DRAG_NOTICE_TIMEOUT, summary.fields())
            .is_err()
        {
            log::info!("Flow: the drag notice didn't reach {pid}");
        }
    }

    fn cross(
        &self,
        pid: &str,
        side: &'static str,
        x: f64,
        y: f64,
        rect: crate::geometry::Rect,
        dragged: Option<Vec<PathBuf>>,
    ) -> bool {
        let Some(peer) = self.store.peer(pid) else {
            return false;
        };
        let name = peer.name.clone();
        let (pointers, keyboards) = self.targets(pid);
        if pointers.is_empty() {
            if self
                .devices()
                .iter()
                .any(|d| d.online && is_pointer(d.kind))
            {
                self.notice(
                    Text::new(n_!(
                        "Choose which channel of your mouse {name} is on (Flow → Channels)"
                    ))
                    .arg("name", plain(name)),
                    "warn",
                    Some(format!("channel:{pid}")),
                    Duration::from_secs(10),
                );
            }
            // else: the mouse is not here (e.g. a touchpad pushed the edge): nothing to move
            return false;
        }
        let mut rects = self.screens();
        if rects.is_empty() {
            rects.push(rect);
        }
        let t = relative(&rects, side, x, y);
        let devices: Vec<Value> = pointers
            .iter()
            .chain(keyboards.iter())
            .take(8)
            .map(|(d, _)| {
                let kind = match kind_name(d.kind) {
                    k @ ("mouse" | "trackball" | "trackpad" | "keyboard") => k,
                    _ => "unknown",
                };
                json!({"name": clean_name(&d.name), "kind": kind})
            })
            .collect();
        let t_wire = (t * 1e5).round_ties_even() / 1e5;
        if let Some(paths) = &dragged {
            self.drag_notice(pid, paths);
        }
        let answer = match self.link(pid) {
            None => None,
            Some(link) => link
                .request(
                    "switch",
                    SWITCH_TIMEOUT,
                    json!({"side": side, "t": t_wire, "devices": devices}),
                )
                .ok(),
        };
        let Some(r) = answer else {
            self.notice(
                Text::new(n_!("Can't reach {name}: the mouse stays on this computer"))
                    .arg("name", plain(name)),
                "warn",
                Some(format!("unreachable:{pid}")),
                Duration::from_secs(5),
            );
            return false;
        };
        if r["type"] != "switch_ok" {
            let reason = r
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("protocol");
            let msgid = refused_text(reason)
                .or(refused_text("protocol"))
                .expect("known reason");
            let why = Text::new(msgid)
                .arg("name", plain(name))
                .arg("app", glisa_common::APP_NAME);
            self.notice(
                Text::new(n_!("{reason}: the mouse stays on this computer")).arg("reason", why),
                "warn",
                Some(format!("refused:{pid}")),
                Duration::from_secs(5),
            );
            return false;
        }
        if dragged.is_some() {
            // Take the drop (as a copy) before the switch lets go of the button. If the
            // button was let go already, the drop was refused: nothing crosses.
            let still = self.backend().is_some_and(|b| b.drag_accept(true));
            if !still {
                log::info!("Flow: the drag ended before the switch; the mouse stays");
                self.hide_catcher();
                return false;
            }
        }
        // The peer expects the device: let go of anything we hold, then switch.
        let host = self.host();
        if let Some(h) = &host {
            h.release_all_input();
        }
        if let Some(h) = host {
            let (tx, rx) = mpsc::channel();
            let all: Vec<(FlowDevice, u8)> =
                keyboards.iter().chain(pointers.iter()).cloned().collect();
            let n = all.len();
            for (d, ch) in all {
                let (h, tx) = (h.clone(), tx.clone());
                std::thread::spawn(move || {
                    if let Err(e) = h.set_host(&d.id, ch) {
                        log::warn!(
                            "Flow: could not switch {} to channel {}: {e}",
                            d.name,
                            ch + 1
                        );
                    }
                    let _ = tx.send(());
                });
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            for _ in 0..n {
                if rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .is_err()
                {
                    break;
                }
            }
        }
        {
            let mut st = lock(&self.st);
            st.cooldown_until = Instant::now()
                + Duration::from_millis(self.store.options().cooldown_ms.max(0) as u64);
            st.at_edge = None;
            st.last_switch = Some(json!({"peer": pid, "side": side, "t": t,
                "devices": devices.iter().map(|d| d["name"].clone()).collect::<Vec<_>>(), "at": unix_time()}));
        }
        self.emit_flow("switch");
        let svc = self.arc();
        let pid = pid.to_string();
        match dragged {
            Some(paths) => {
                self.hide_catcher(); // the backend keeps it until the drop has landed
                self.hide_glow();
                let _ = std::thread::Builder::new()
                    .name("flow-drag".into())
                    .spawn(move || {
                        if let Err(e) = svc.send_paths(&pid, paths, SendReason::Drag) {
                            log::info!("Flow: the dragged files weren't sent: {e}");
                        }
                    });
            }
            None => {
                let _ = std::thread::Builder::new()
                    .name("flow-clipboard".into())
                    .spawn(move || svc.after_switch(&pid));
            }
        }
        true
    }

    pub(crate) fn incoming_switch(&self, pid: &str, m: &Msg) -> Result<(), &'static str> {
        if !self.running() {
            return Err("disabled");
        }
        let their = m["side"].as_str().unwrap_or("left");
        let side = self
            .store
            .layout()
            .get(pid)
            .and_then(|s| side_of(s))
            .unwrap_or_else(|| opposite(their));
        let names: Vec<String> = m["devices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|d| {
                        d["kind"]
                            .as_str()
                            .is_some_and(|k| POINTER_KINDS.contains(&k))
                    })
                    .filter_map(|d| d["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let now = Instant::now();
        {
            let mut st = lock(&self.st);
            st.pending = Some(Pending {
                peer: pid.to_string(),
                side,
                t: m["t"].as_f64().unwrap_or(0.5),
                names,
                until: now + ARRIVAL_WINDOW,
                since: now,
            });
            st.cooldown_until =
                now + Duration::from_millis(self.store.options().cooldown_ms.max(0) as u64);
        }
        self.expect_pickup(pid, side, m["t"].as_f64().unwrap_or(0.5));
        self.watch_arrival();
        Ok(())
    }

    /// Receivers keep their node while the device is away and Bluetooth nodes only
    /// reappear on the next scan: ask the host to look every 0.3 s while a device is on
    /// its way (real hardware only), like the Python service did.
    fn watch_arrival(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        if self.demo || self.arrival_watch.swap(true, SeqCst) {
            return;
        }
        let me = self.me.clone();
        let spawned = std::thread::Builder::new()
            .name("flow-arrival".into())
            .spawn(move || {
                loop {
                    let Some(svc) = me.upgrade() else { return };
                    let waiting = svc.running() && lock(&svc.st).pending.is_some();
                    if !waiting {
                        svc.arrival_watch.store(false, SeqCst);
                        // A crossing may have started between the check and the store.
                        let again = svc.running() && lock(&svc.st).pending.is_some();
                        if !again || svc.arrival_watch.swap(true, SeqCst) {
                            return;
                        }
                    }
                    if let Some(host) = svc.host() {
                        host.poll_arrivals();
                    }
                    drop(svc);
                    std::thread::sleep(Duration::from_millis(300));
                }
            });
        if spawned.is_err() {
            self.arrival_watch.store(false, SeqCst);
        }
    }

    /// Tests and the fake backend: expect a device as if a peer had said `switch`.
    #[doc(hidden)]
    pub fn expect_arrival_for_tests(&self, side: &str, t: f64, names: &[&str]) -> bool {
        let devices: Vec<Value> = names
            .iter()
            .map(|n| json!({"name": n, "kind": "mouse"}))
            .collect();
        let m = json!({"side": side, "t": t, "devices": devices});
        self.incoming_switch("peer-x", m.as_object().expect("object"))
            .is_ok()
    }

    pub fn has_pending_arrival(&self) -> bool {
        lock(&self.st).pending.is_some()
    }

    /// A Logitech node just appeared, before its HID++ set-up: the earliest sign that an
    /// expected device has arrived. Receivers keep their node, so they never count.
    pub(crate) fn appearing(&self, info: &AppearingDevice) {
        if info.is_receiver {
            return;
        }
        let pending = {
            let mut st = lock(&self.st);
            st.last_appearing = Some(Instant::now());
            st.pending.is_some()
        };
        if self.running() && pending {
            self.post(Item::Appearing);
        }
    }

    pub(crate) fn online(&self, device_id: &str) {
        if self.running() && lock(&self.st).pending.is_some() {
            self.post(Item::Online(device_id.to_string()));
        }
    }

    fn take_pending(&self, now: Instant) -> Option<Pending> {
        let p = {
            let mut st = lock(&self.st);
            if st.pending.as_ref().is_none_or(|p| now > p.until) {
                return None;
            }
            let p = st.pending.take();
            st.cooldown_until =
                now + Duration::from_millis(self.store.options().cooldown_ms.max(0) as u64);
            for s in SIDES {
                st.armed.insert(s, true);
            }
            st.at_edge = None;
            p
        };
        self.pickup_arrived(); // the mouse is here: a drag coming with it can go on
        p
    }

    /// Fast path: place the pointer as soon as the device's node shows up.
    fn appearing_now(&self) {
        if let Some(p) = self.take_pending(Instant::now()) {
            self.place(&p);
        }
    }

    fn online_now(&self, device_id: &str) {
        let Some(dev) = self
            .host()
            .and_then(|h| h.flow_devices().into_iter().find(|d| d.id == device_id))
        else {
            return;
        };
        // When the device became usable: a node that appeared during this arrival (and
        // wasn't placed by the fast path) counts from then; otherwise from now.
        let appeared = {
            let st = lock(&self.st);
            match (&st.pending, st.last_appearing) {
                (Some(p), Some(t)) if t >= p.since => Some(t),
                _ => None,
            }
        };
        self.arrived(&dev, appeared);
    }

    /// The device `dev` is online here; `appeared` is when it became usable (None: now).
    #[doc(hidden)]
    pub fn arrived(&self, dev: &FlowDevice, appeared: Option<Instant>) {
        let now = Instant::now();
        {
            let st = lock(&self.st);
            let Some(p) = &st.pending else { return };
            if now > p.until || !is_pointer(dev.kind) {
                return;
            }
            if !p.names.is_empty()
                && !p.names.contains(&dev.name)
                && !p.names.contains(&clean_name(&dev.name))
            {
                return;
            }
        }
        let Some(p) = self.take_pending(now) else {
            return;
        };
        if let Some(t) = appeared {
            if now.duration_since(t) > LATE_PLACEMENT {
                // The user has been moving the mouse for a while: leave the pointer alone.
                log::info!(
                    "Flow: {} became usable {:.1} s ago; not moving the pointer",
                    dev.name,
                    now.duration_since(t).as_secs_f64()
                );
                lock(&self.st).last_arrival = Some(
                    json!({"peer": p.peer, "side": p.side, "placed": false, "late": true, "at": unix_time()}),
                );
                self.emit_flow("arrival");
                return;
            }
        }
        self.place(&p);
    }

    fn place(&self, p: &Pending) {
        let (x, y) = entry_point(&self.screens(), p.side, p.t);
        let ok = self.backend().is_some_and(|b| b.place_cursor(x, y));
        lock(&self.st).last_arrival = Some(
            json!({"peer": p.peer, "side": p.side, "x": x, "y": y, "placed": ok, "at": unix_time()}),
        );
        self.emit_flow("arrival");
        if !ok {
            self.notice(
                Text::new(n_!(
                    "The mouse arrived, but the pointer couldn't be placed at the edge"
                )),
                "warn",
                Some("place".into()),
                Duration::from_secs(60),
            );
        }
    }

    // ============================================================== clipboard
    /// Where clipboard text is read and written ("gui": the app's fallback).
    pub(crate) fn clipboard_kind(&self) -> &'static str {
        match self.backend().map(|b| b.clipboard_kind()) {
            Some(k) if k != "none" => k,
            _ => "gui",
        }
    }

    fn clipboard_get(&self) -> Option<String> {
        if let Some(b) = self.backend() {
            if b.clipboard_kind() != "none" {
                return b.clipboard_get(Duration::from_millis(500));
            }
        }
        let offer = lock(&self.st).gui_clip_offer.clone();
        (!offer.is_empty()).then_some(offer)
    }

    pub(crate) fn send_clipboard(&self, pid: &str) {
        let Some(text) = self.clipboard_get() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if text.len() > CLIPBOARD_MAX {
            self.notice(
                Text::new(n_!(
                    "The copied text is too large to send to the other computer (1 MB maximum)"
                )),
                "info",
                Some("clipsize".into()),
                Duration::from_secs(30),
            );
            return;
        }
        let h = crate::transfer::text_key(&text);
        if lock(&self.st).clip_hash.get(pid) == Some(&h) {
            return;
        }
        let Some(link) = self.link(pid) else { return };
        match link.request("clipboard", Duration::from_secs(4), json!({"text": text})) {
            Ok(_) => {
                lock(&self.st).clip_hash.insert(pid.to_string(), h);
            }
            Err(_) => log::info!("Flow: could not send the clipboard"),
        }
    }

    pub(crate) fn incoming_clipboard(&self, pid: &str, text: &str) {
        if !self.store.options().clipboard {
            return;
        }
        lock(&self.st)
            .clip_hash
            .insert(pid.to_string(), crate::transfer::text_key(text)); // no echo back
        if let Some(b) = self.backend() {
            if b.clipboard_kind() != "none" && b.clipboard_set(text) {
                return;
            }
        }
        {
            let mut st = lock(&self.st);
            st.gui_clip_seq += 1;
            st.gui_clip_text = text.to_string();
        }
        self.emit_flow("clipboard");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(entries: Value, current: u8) -> FlowDevice {
        FlowDevice {
            id: "b031-lift".into(),
            key: "B031:LIFT".into(),
            name: "LIFT".into(),
            kind: DeviceKind::Mouse,
            online: true,
            hosts_count: 3,
            current_host: current,
            host_entries: entries.as_array().unwrap().clone(),
        }
    }

    #[test]
    fn channel_suggestion() {
        let d = dev(
            json!([{"index": 0, "paired": true, "name": "equipo-a"}, {"index": 1, "paired": true, "name": "Equipo-B.local"},
                   {"index": 2, "paired": false, "name": ""}]),
            0,
        );
        assert_eq!(suggest_channel(&d, "equipo-b"), Some(1));
        assert_eq!(suggest_channel(&d, "EQUIPO_B"), Some(1));
        assert_eq!(suggest_channel(&d, "equipo-a"), None); // the current channel
        assert_eq!(suggest_channel(&d, "equipo"), Some(1)); // prefix
        assert_eq!(suggest_channel(&d, "mac"), None);
        assert_eq!(suggest_channel(&d, ""), None);
    }
}
