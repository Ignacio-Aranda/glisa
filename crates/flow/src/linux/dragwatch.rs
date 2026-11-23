//! Files dragged anywhere on the desktop (KDE Plasma on Wayland), so the edges that lead to
//! another computer can glow while the drag lasts (docs/FLOW-DRAG-DROP.md).
//!
//! A Wayland client never sees other clients' drags, so the watch asks the compositor at the
//! one moment a drag can have started:
//!
//! 1. the left button of the Logitech mouse goes down ([`PointerSource`]: its evdev node,
//!    filtered by the kernel to that button and, from now until this press is decided, its
//!    motion);
//! 2. once it has moved [`PROBE_AFTER`] device units with the button held (the usual drag
//!    distance) and some edge would glow, the catcher puts up its **probe**: invisible
//!    surfaces over the outputs for at most [`PROBE_WINDOW`]
//!    ([`super::catcher::Catcher::probe`]). A drag and drop in progress enters it; an
//!    ordinary press (a selection, a window being moved) keeps its implicit grab and never
//!    does. The probe goes the moment a drag enters it, and never takes a drop or data;
//! 3. a drag that offers `text/uri-list` is a file drag: reported until the button is let go.
//!
//! A press whose drag hadn't started yet when the first probe looked gets a second chance
//! further on; never more than [`PROBE_AFTER`]`.len()` probes per press.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use super::input::{PointerEvent, PointerSource, Wake};

/// Motion since the press (device units, |dx| + |dy|) before each probe.
pub const PROBE_AFTER: [i64; 2] = [12, 48];
/// How long a probe waits for a drag to enter it, once it is on screen.
pub const PROBE_WINDOW: Duration = Duration::from_millis(150);

/// What the watch asks and tells (the backend wires it to the catcher and the engine).
pub trait DragEvents: Send + Sync {
    /// Would a file drag glow anywhere now? No: no probe for this press.
    fn wanted(&self) -> bool;
    /// Puts up the probe for `window`: Some(true) a file drag entered it, Some(false) another
    /// drag did, None nothing did.
    fn probe(&self, window: Duration) -> Option<bool>;
    /// A file drag started (true), or the button holding it was let go (false).
    fn dragging(&self, on: bool);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// The left button is down: `moved` so far, `probes` done.
    Pressed {
        moved: i64,
        probes: usize,
    },
    /// This press is decided and isn't a file drag.
    Decided,
    /// A file drag, reported.
    Dragging,
}

/// The state machine, apart from the thread (tests drive it directly).
pub struct Watch {
    phase: Phase,
}

impl Default for Watch {
    fn default() -> Self {
        Self { phase: Phase::Idle }
    }
}

impl Watch {
    pub fn dragging(&self) -> bool {
        self.phase == Phase::Dragging
    }

    pub fn feed(&mut self, ev: PointerEvent, src: &mut dyn PointerSource, to: &dyn DragEvents) {
        match ev {
            PointerEvent::Left(true) => {
                if self.phase == Phase::Dragging {
                    to.dragging(false); // the release went unseen
                }
                self.phase = Phase::Pressed {
                    moved: 0,
                    probes: 0,
                };
                src.want_motion(true);
            }
            PointerEvent::Left(false) => {
                if self.phase == Phase::Dragging {
                    to.dragging(false);
                }
                if matches!(self.phase, Phase::Pressed { .. }) {
                    src.want_motion(false);
                }
                self.phase = Phase::Idle;
            }
            PointerEvent::Motion(dx, dy) => {
                let Phase::Pressed { moved, probes } = self.phase else {
                    return;
                };
                let moved = moved + (dx as i64).abs() + (dy as i64).abs();
                self.phase = Phase::Pressed { moved, probes };
                if probes >= PROBE_AFTER.len() || moved < PROBE_AFTER[probes] {
                    return;
                }
                let probes = probes + 1;
                let decided = if !to.wanted() {
                    Some(Phase::Decided) // no edge would glow: nothing to find out
                } else {
                    match to.probe(PROBE_WINDOW) {
                        Some(true) => Some(Phase::Dragging),
                        Some(false) => Some(Phase::Decided), // text, a link…: no glow
                        None if probes == PROBE_AFTER.len() => Some(Phase::Decided),
                        None => None,
                    }
                };
                match decided {
                    Some(p) => {
                        self.phase = p;
                        src.want_motion(false);
                        if p == Phase::Dragging {
                            to.dragging(true);
                        }
                    }
                    None => self.phase = Phase::Pressed { moved, probes },
                }
            }
        }
    }
}

/// The watch's thread: events from `source` until stopped.
pub struct DragWatch {
    stop: Arc<AtomicBool>,
    wake: Option<Wake>,
    thread: Option<JoinHandle<()>>,
}

impl DragWatch {
    /// `wake` wakes `source` up when the watch stops (None: `source` returns often enough).
    pub fn start(
        mut source: Box<dyn PointerSource>,
        wake: Option<Wake>,
        to: Arc<dyn DragEvents>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("flow-drag-watch".into())
            .spawn(move || {
                let mut w = Watch::default();
                while !s2.load(Ordering::SeqCst) {
                    let events = match source.next(Duration::from_secs(60)) {
                        Ok(e) => e,
                        Err(e) => {
                            log::info!("Flow: the drag watch stopped: {e}");
                            break;
                        }
                    };
                    for ev in events {
                        if s2.load(Ordering::SeqCst) {
                            break;
                        }
                        w.feed(ev, source.as_mut(), to.as_ref());
                    }
                }
                if w.dragging() {
                    to.dragging(false);
                }
            })
            .ok();
        Self { stop, wake, thread }
    }

    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(w) = &self.wake {
            w.wake();
        }
        if let Some(t) = self.thread.take() {
            if t.thread().id() != std::thread::current().id() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for DragWatch {
    fn drop(&mut self) {
        self.halt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;

    use PointerEvent::{Left, Motion};

    #[derive(Default)]
    struct Rec {
        log: Mutex<Vec<String>>,
        wanted: AtomicBool,
        /// What each probe finds, in turn (then None).
        finds: Mutex<Vec<Option<bool>>>,
    }

    impl Rec {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.log.lock().unwrap())
        }
    }

    impl DragEvents for Rec {
        fn wanted(&self) -> bool {
            self.wanted.load(Ordering::SeqCst)
        }
        fn probe(&self, window: Duration) -> Option<bool> {
            assert_eq!(window, PROBE_WINDOW);
            let mut f = self.finds.lock().unwrap();
            let r = if f.is_empty() { None } else { f.remove(0) };
            self.log.lock().unwrap().push(format!("probe {r:?}"));
            r
        }
        fn dragging(&self, on: bool) {
            self.log.lock().unwrap().push(format!("dragging {on}"));
        }
    }

    /// Records the motion requests; events come from a channel.
    struct Src(Arc<Mutex<Vec<bool>>>, mpsc::Receiver<PointerEvent>);

    impl PointerSource for Src {
        fn next(&mut self, timeout: Duration) -> std::io::Result<Vec<PointerEvent>> {
            match self.1.recv_timeout(timeout.min(Duration::from_millis(20))) {
                Ok(e) => Ok(vec![e]),
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(Vec::new()),
                Err(_) => Err(std::io::Error::other("closed")),
            }
        }
        fn want_motion(&mut self, on: bool) {
            self.0.lock().unwrap().push(on);
        }
    }

    fn src() -> (Src, Arc<Mutex<Vec<bool>>>, mpsc::Sender<PointerEvent>) {
        let (tx, rx) = mpsc::channel();
        let m = Arc::new(Mutex::new(Vec::new()));
        (Src(m.clone(), rx), m, tx)
    }

    #[test]
    fn a_file_drag_is_found_once_the_pointer_has_moved() {
        let (mut s, motion, _tx) = src();
        let rec = Rec::default();
        rec.wanted.store(true, Ordering::SeqCst);
        *rec.finds.lock().unwrap() = vec![Some(true)];
        let mut w = Watch::default();
        w.feed(Left(true), &mut s, &rec);
        assert_eq!(*motion.lock().unwrap(), vec![true]); // motion only from the press on
        w.feed(Motion(3, -4), &mut s, &rec);
        w.feed(Motion(4, 0), &mut s, &rec);
        assert!(rec.take().is_empty()); // 11 units: not yet a drag
        w.feed(Motion(0, 1), &mut s, &rec);
        assert_eq!(rec.take(), ["probe Some(true)", "dragging true"]);
        assert_eq!(*motion.lock().unwrap(), vec![true, false]); // decided: no more motion
        w.feed(Motion(100, 100), &mut s, &rec);
        assert!(rec.take().is_empty());
        w.feed(Left(false), &mut s, &rec);
        assert_eq!(rec.take(), ["dragging false"]);
        assert_eq!(*motion.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn plain_presses_text_drags_and_nothing_to_glow() {
        let (mut s, motion, _tx) = src();
        let rec = Rec::default();
        let mut w = Watch::default();
        // no edge would glow: never a probe
        w.feed(Left(true), &mut s, &rec);
        w.feed(Motion(30, 0), &mut s, &rec);
        w.feed(Motion(300, 0), &mut s, &rec);
        w.feed(Left(false), &mut s, &rec);
        assert!(rec.take().is_empty());
        assert_eq!(*motion.lock().unwrap(), vec![true, false]);
        rec.wanted.store(true, Ordering::SeqCst);
        // a selection: nothing enters the probe, twice (a second chance further on)
        w.feed(Left(true), &mut s, &rec);
        w.feed(Motion(12, 0), &mut s, &rec);
        assert_eq!(rec.take(), ["probe None"]);
        w.feed(Motion(30, 0), &mut s, &rec);
        assert!(rec.take().is_empty());
        w.feed(Motion(6, 0), &mut s, &rec);
        assert_eq!(rec.take(), ["probe None"]);
        w.feed(Motion(500, 0), &mut s, &rec);
        w.feed(Left(false), &mut s, &rec);
        assert!(rec.take().is_empty());
        // a drag of text: decided at once, no glow
        *rec.finds.lock().unwrap() = vec![Some(false)];
        w.feed(Left(true), &mut s, &rec);
        w.feed(Motion(0, 20), &mut s, &rec);
        w.feed(Motion(0, 200), &mut s, &rec);
        w.feed(Left(false), &mut s, &rec);
        assert_eq!(rec.take(), ["probe Some(false)"]);
        // the drag started late: found by the second probe
        *rec.finds.lock().unwrap() = vec![None, Some(true)];
        w.feed(Left(true), &mut s, &rec);
        w.feed(Motion(20, 0), &mut s, &rec);
        w.feed(Motion(40, 0), &mut s, &rec);
        assert_eq!(
            rec.take(),
            ["probe None", "probe Some(true)", "dragging true"]
        );
        // a press without a release in between (missed): the old drag is over
        w.feed(Left(true), &mut s, &rec);
        assert_eq!(rec.take(), ["dragging false"]);
        // motion without a press means nothing
        w.feed(Left(false), &mut s, &rec);
        w.feed(Motion(100, 0), &mut s, &rec);
        assert!(rec.take().is_empty());
    }

    #[test]
    fn the_thread_reports_and_stops() {
        let (s, _motion, tx) = src();
        let rec = Arc::new(Rec::default());
        rec.wanted.store(true, Ordering::SeqCst);
        *rec.finds.lock().unwrap() = vec![Some(true)];
        let w = DragWatch::start(Box::new(s), None, rec.clone());
        for e in [Left(true), Motion(8, 8)] {
            tx.send(e).unwrap();
        }
        let found = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            rec.log.lock().unwrap().len() == 2
        });
        assert!(found, "{:?}", rec.log.lock().unwrap());
        // stopped in the middle of a drag: it is over for the engine
        w.stop();
        assert_eq!(
            rec.take(),
            ["probe Some(true)", "dragging true", "dragging false"]
        );
    }
}
