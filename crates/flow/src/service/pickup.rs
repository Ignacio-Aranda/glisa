//! Files dragged here from another computer (docs/FLOW.md §6b).
//!
//! The sender's `drag` notice (just before its `switch`) says what is coming. When that
//! `switch` follows, the backend puts up the **pickup** (KDE Plasma on Wayland): an invisible
//! surface over the output where the pointer will enter. The mouse arrives with its button
//! still held; that press, if the mouse itself says its left button is down (even before its
//! arrival is noticed, as behind a receiver), starts a drag of the received files right there,
//! so the user simply keeps dragging and drops them wherever they like (copy only).
//!
//! The transfer the notice announced (same peer, its first top-level name among the
//! announced ones) carries the dragged files. While the pickup may still take them, they wait
//! to go on the clipboard; a drop that gets them (on KDE, one the drag copied them for:
//! `linux/arkdrop.rs`) ends it there. Otherwise (no press within
//! [`PICKUP_PRESS`] of the arrival, a cancelled drag, files not all here when dropped, or a
//! desktop without pickup) they are received exactly like copied files: on the clipboard
//! for Ctrl+V, with the usual notice.
//!
//! The pickup surface takes every click on that output while it is up, so it never stays:
//! at most [`PICKUP_ARRIVAL`] waiting for the mouse, then [`PICKUP_PRESS`] for its press. A
//! pickup that saw no mouse arrive holds the next one off for [`PICKUP_COOLDOWN`], and none
//! goes up while a mouse of ours is here (no drag can be coming with it then).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::FlowService;
use super::transfers::Held;
use crate::backends::{PickupNews, PickupStep};
use crate::geometry::{entry_point, output_at};
use crate::proto::Msg;
use crate::util::lock;

/// How long a notice waits for its transfer (the sender hashes the files first).
pub const DRAG_WAIT: Duration = Duration::from_secs(120);
/// The `switch` that follows a notice comes within this.
const NOTICE_FRESH: Duration = Duration::from_secs(5);
/// How long the pickup waits for our mouse after the switch.
pub const PICKUP_ARRIVAL: Duration = Duration::from_secs(8);
/// How long it waits for the press once the mouse is here.
pub const PICKUP_PRESS: Duration = Duration::from_secs(3);
/// A drag picked up here is over after this at the latest.
const PICKUP_DRAG: Duration = Duration::from_secs(120);
/// No pickup for this long after one whose mouse never came.
const PICKUP_COOLDOWN: Duration = Duration::from_secs(30);

/// The last `drag` notice: what is coming.
pub(crate) struct IncomingDrag {
    peer: String,
    names: Vec<String>,
    count: usize,
    since: Instant,
    /// Its switch came (one pickup per notice).
    switched: bool,
    /// Its transfer came (one transfer per notice).
    bound: bool,
}

/// A drag that may be picked up here.
pub(crate) struct Pickup {
    peer: String,
    /// Over at this point unless something happens first.
    until: Instant,
    arrived: bool,
    started: bool,
    /// The dragged files, here, waiting for the outcome before the clipboard.
    held: Option<Held>,
}

impl FlowService {
    /// A peer says files it dragged across the edge are coming with the mouse.
    pub(crate) fn incoming_drag(&self, pid: &str, m: &Msg) {
        let opts = self.store.options();
        if !self.running() || !opts.files || !opts.drag_files {
            return; // turned off here: they come as copied files
        }
        let names: Vec<String> = m["names"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let count = m["count"].as_u64().unwrap_or(1).min(1 << 20) as usize;
        lock(&self.st).incoming_drag = Some(IncomingDrag {
            peer: pid.to_string(),
            names,
            count,
            since: Instant::now(),
            switched: false,
            bound: false,
        });
        log::info!("Flow: {pid} is dragging {count} item(s) here");
    }

    /// Is the transfer `pid` starts the one its last `drag` notice announced? (Same peer,
    /// files, its first top-level name among the announced ones, not too late; once.)
    pub(crate) fn drag_expects(&self, pid: &str, kind: &str, entries: &[Value]) -> bool {
        let mut st = lock(&self.st);
        let Some(d) = st.incoming_drag.as_mut() else {
            return false;
        };
        let first = entries
            .iter()
            .filter_map(|e| e["path"].as_str())
            .find(|p| !p.contains('/'));
        let ok = d.peer == pid
            && !d.bound
            && kind == "files"
            && d.since.elapsed() < DRAG_WAIT
            && first.is_some_and(|f| d.names.iter().any(|n| n == f));
        if ok {
            d.bound = true;
        }
        ok
    }

    /// The `switch` after a `drag` notice: catch the arriving mouse's press on the output
    /// where the pointer will enter (`side`, `t` of this desktop).
    pub(crate) fn expect_pickup(&self, pid: &str, side: &'static str, t: f64) {
        // A mouse of ours is here already: no drag is coming with it (and a peer can't make
        // its clicks go nowhere).
        let here = self
            .devices()
            .iter()
            .any(|d| d.online && super::engine::is_pointer(d.kind));
        let count = {
            let mut st = lock(&self.st);
            let now = Instant::now();
            let Some(d) = st
                .incoming_drag
                .as_mut()
                .filter(|d| d.peer == pid && !d.switched && d.since.elapsed() < NOTICE_FRESH)
            else {
                return;
            };
            d.switched = true;
            let count = d.count;
            if here
                || st.pickup.as_ref().is_some_and(|p| p.started)
                || st.pickup_cooldown.is_some_and(|t| now < t)
            {
                return;
            }
            count
        };
        let Some(backend) = self.backend().filter(|b| b.drag_pickup_available()) else {
            return; // they come as copied files
        };
        let screens = self.screens();
        let (x, y) = entry_point(&screens, side, t);
        let output = output_at(&screens, x as f64, y as f64)
            .or_else(|| screens.first().copied())
            .unwrap_or(crate::geometry::DEFAULT_SCREEN);
        let old = lock(&self.st).pickup.replace(Pickup {
            peer: pid.to_string(),
            until: Instant::now() + PICKUP_ARRIVAL,
            arrived: false,
            started: false,
            held: None,
        });
        if let Some(h) = old.and_then(|p| p.held) {
            self.place_received(h);
        }
        log::info!("Flow: waiting for the mouse to pick the dragged files up on {output:?}");
        backend.drag_pickup(PickupStep::Expect { output, count });
    }

    /// Our mouse arrived: its press counts from now.
    pub(crate) fn pickup_arrived(&self) {
        let wait = lock(&self.hooks).pickup_press_wait.unwrap_or(PICKUP_PRESS);
        let armed = {
            let mut st = lock(&self.st);
            match st.pickup.as_mut().filter(|p| !p.arrived) {
                Some(p) => {
                    p.arrived = true;
                    p.until = Instant::now() + wait;
                    true
                }
                None => false,
            }
        };
        if armed {
            log::info!("Flow: the mouse is here; a press of its left button picks the drag up");
            if let Some(b) = self.backend() {
                b.drag_pickup(PickupStep::Arrived);
            }
        }
    }

    /// News from the backend.
    pub(crate) fn pickup_news(&self, news: PickupNews) {
        match news {
            PickupNews::Started => {
                let mut st = lock(&self.st);
                if let Some(p) = st.pickup.as_mut() {
                    p.started = true;
                    p.until = Instant::now() + PICKUP_DRAG;
                    log::info!("Flow: the drag goes on here");
                }
            }
            PickupNews::Ended { taken } => self.end_pickup(taken),
        }
    }

    /// No press in time, or a drag that never ended: the pickup is over.
    pub(crate) fn expire_pickup(&self) {
        let over = lock(&self.st)
            .pickup
            .as_ref()
            .is_some_and(|p| Instant::now() >= p.until);
        if over {
            if let Some(b) = self.backend() {
                b.drag_pickup(PickupStep::Cancel);
            }
            self.end_pickup(false);
        }
    }

    fn end_pickup(&self, taken: bool) {
        let Some(p) = lock(&self.st).pickup.take() else {
            return;
        };
        if !p.arrived {
            lock(&self.st).pickup_cooldown = Some(Instant::now() + PICKUP_COOLDOWN);
        }
        let how = match (taken, p.started, p.arrived) {
            (true, _, _) => "dropped",
            (false, true, _) => "not taken (cancelled, or the files weren't all here)",
            (false, false, true) => "no press came",
            (false, false, false) => "the mouse didn't arrive",
        };
        log::info!("Flow: picking the drag up here: {how}");
        match p.held {
            Some(h) if taken => self.finish_dropped(h),
            Some(h) => self.place_received(h), // as copied files: Ctrl+V
            None => {}
        }
    }

    /// The dragged files arrived: they wait for the pickup if it may still take them (and it
    /// is told they are ready); otherwise they come back to go on the clipboard now.
    pub(crate) fn pickup_hold(&self, held: Held, paths: &[PathBuf]) -> Option<Held> {
        {
            let mut st = lock(&self.st);
            let Some(p) = st
                .pickup
                .as_mut()
                .filter(|p| p.peer == held.peer() && p.held.is_none())
            else {
                return Some(held);
            };
            p.held = Some(held);
        }
        if let Some(b) = self.backend() {
            b.drag_pickup(PickupStep::Ready(paths.to_vec()));
        }
        None
    }

    /// Flow stops: a pickup still holding files lets them go to the clipboard.
    pub(crate) fn drop_pickup(&self) {
        let p = {
            let mut st = lock(&self.st);
            st.incoming_drag = None;
            st.pickup.take()
        };
        if let Some(h) = p.and_then(|p| p.held) {
            self.place_received(h);
        }
    }
}
