//! Linux input devices for Flow on KDE Wayland:
//!
//! * [`AbsolutePointer`]: a second virtual device (uinput) that places the cursor where the
//!   mouse arrives — EV_ABS `ABS_X`/`ABS_Y` 0…65535 + `BTN_LEFT/RIGHT/MIDDLE`, no
//!   `BTN_TOUCH`, so udev tags it `ID_INPUT_MOUSE` and libinput treats it as an absolute
//!   pointer. KWin maps the whole range onto the bounding box of all outputs from the global
//!   origin (docs/FLOW.md §5), so one device addresses every monitor.
//! * [`MouseButtons`]: whether a button of the Logitech mouse is held right now, from its
//!   evdev node (`EVIOCGKEY`), asked only when the pointer is at an edge: the KWin script
//!   can't see buttons, and a drag to the edge must never switch. It needs read access to
//!   that node (a udev `uaccess` rule for Logitech mice); without it the state is unknown
//!   and Flow behaves as before.
//! * [`EvdevPointer`]: the left button of the Logitech mouse, as events, and its motion only
//!   from a left press until the drag watch has decided whether files are being dragged
//!   (`linux/dragwatch.rs`). The kernel filters for us (`EVIOCSMASK`): nothing else of the
//!   device ever reaches this process, and a node that can't be filtered is not read.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::backends::{ABS_MAX, ABS_X, ABS_Y, EV_ABS, EV_SYN, SYN_REPORT};
use crate::util::lock;

const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const SYN_DROPPED: u16 = 3;
const EV_CNT: usize = 0x20;
const KEY_CNT: usize = 0x300;
const REL_CNT: usize = 0x10;
/// Letter keys (Q, A, Z): a node that has them is a keyboard, never read.
const LETTERS: [usize; 3] = [16, 30, 44];
/// `_IOW('E', 0x93, struct input_mask)`.
const EVIOCSMASK: libc::c_ulong = 0x4010_4593;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_ABSBIT: libc::c_ulong = 0x4004_5567;
/// _IOW('U', 4, struct uinput_abs_setup)
const UI_ABS_SETUP: libc::c_ulong = 0x401C_5504;
/// _IOW('U', 3, struct uinput_setup)
const UI_DEV_SETUP: libc::c_ulong = 0x405C_5503;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
const BUS_VIRTUAL: u16 = 0x06;
const KEY_MAX: usize = 0x2ff;

fn ioctl_int(f: &File, req: libc::c_ulong, arg: libc::c_ulong) -> io::Result<()> {
    // SAFETY: an int-argument uinput ioctl on a file we own.
    let r = unsafe { libc::ioctl(f.as_raw_fd(), req as _, arg) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ioctl_ptr(f: &File, req: libc::c_ulong, data: &mut [u8]) -> io::Result<()> {
    // SAFETY: `data` is as large as the struct the request reads or writes.
    let r = unsafe { libc::ioctl(f.as_raw_fd(), req as _, data.as_mut_ptr()) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub struct AbsolutePointer {
    f: Mutex<File>,
}

impl AbsolutePointer {
    pub fn new(name: &str) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/uinput")?;
        ioctl_int(&f, UI_SET_EVBIT, EV_KEY as _)?;
        ioctl_int(&f, UI_SET_EVBIT, EV_ABS as _)?;
        for code in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
            ioctl_int(&f, UI_SET_KEYBIT, code as _)?;
        }
        for axis in [ABS_X, ABS_Y] {
            ioctl_int(&f, UI_SET_ABSBIT, axis as _)?;
            // struct uinput_abs_setup { u16 code; (pad) struct input_absinfo {value, min, max, fuzz, flat, res} }
            let mut setup = Vec::with_capacity(28);
            setup.extend_from_slice(&axis.to_ne_bytes());
            setup.extend_from_slice(&[0, 0]);
            for v in [0i32, 0, ABS_MAX, 0, 0, 0] {
                setup.extend_from_slice(&v.to_ne_bytes());
            }
            ioctl_ptr(&f, UI_ABS_SETUP, &mut setup)?;
        }
        // struct uinput_setup { struct input_id {bustype, vendor, product, version}; char name[80]; u32 ff_effects_max }
        let mut setup = Vec::with_capacity(92);
        for v in [BUS_VIRTUAL, 0x1209, 0x4C50, 1] {
            setup.extend_from_slice(&v.to_ne_bytes());
        }
        let mut nm = [0u8; 80];
        let bytes = name.as_bytes();
        nm[..bytes.len().min(79)].copy_from_slice(&bytes[..bytes.len().min(79)]);
        setup.extend_from_slice(&nm);
        setup.extend_from_slice(&0u32.to_ne_bytes());
        ioctl_ptr(&f, UI_DEV_SETUP, &mut setup)?;
        ioctl_int(&f, UI_DEV_CREATE, 0)?;
        std::thread::sleep(Duration::from_millis(200)); // let the compositor pick the device up
        Ok(Self { f: Mutex::new(f) })
    }

    pub fn move_to(&self, ax: i32, ay: i32) -> io::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let mut buf = Vec::with_capacity(72);
        for (t, c, v) in [
            (EV_ABS, ABS_X, ax),
            (EV_ABS, ABS_Y, ay),
            (EV_SYN, SYN_REPORT, 0),
        ] {
            // struct input_event { struct timeval; u16 type; u16 code; s32 value }
            buf.extend_from_slice(&(now.as_secs() as libc::time_t).to_ne_bytes());
            buf.extend_from_slice(&(now.subsec_micros() as libc::suseconds_t).to_ne_bytes());
            buf.extend_from_slice(&t.to_ne_bytes());
            buf.extend_from_slice(&c.to_ne_bytes());
            buf.extend_from_slice(&v.to_ne_bytes());
        }
        lock(&self.f).write_all(&buf)
    }
}

impl Drop for AbsolutePointer {
    fn drop(&mut self) {
        let f = lock(&self.f);
        let _ = ioctl_int(&f, UI_DEV_DESTROY, 0);
    }
}

/// `EVIOCGKEY(len)`: the bitmap of keys/buttons held now.
fn eviocgkey(len: usize) -> libc::c_ulong {
    (2 << 30) | ((len as libc::c_ulong) << 16) | ((b'E' as libc::c_ulong) << 8) | 0x18
}

/// The evdev nodes of Logitech mice (vendor 046d, has BTN_LEFT, not a keyboard).
pub fn logitech_mouse_nodes(sys: &Path) -> Vec<PathBuf> {
    logitech_nodes(sys, false)
}

/// Logitech evdev nodes with a left button; with `pointer`, also relative X/Y motion and
/// none of the letter keys (a mouse, never a keyboard).
fn logitech_nodes(sys: &Path, pointer: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(sys) else {
        return out;
    };
    for e in rd.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("event") {
            continue;
        }
        let dev = e.path().join("device");
        let read = |p: &str| {
            std::fs::read_to_string(dev.join(p))
                .unwrap_or_default()
                .trim()
                .to_lowercase()
        };
        if read("id/vendor") != "046d" {
            continue;
        }
        let keys = read("capabilities/key");
        if !bit_set(&keys, BTN_LEFT as usize) {
            continue;
        }
        if pointer {
            let rel = read("capabilities/rel");
            let motion = bit_set(&rel, REL_X as usize) && bit_set(&rel, REL_Y as usize);
            if !motion || LETTERS.iter().any(|k| bit_set(&keys, *k)) {
                continue;
            }
        }
        out.push(PathBuf::from("/dev/input").join(name));
    }
    out.sort();
    out
}

/// Is bit `n` set in a sysfs capability bitmap ("1f0000 0 0 …", most significant word first)?
fn bit_set(caps: &str, n: usize) -> bool {
    let words: Vec<u64> = caps
        .split_whitespace()
        .filter_map(|w| u64::from_str_radix(w, 16).ok())
        .collect();
    let bits = if usize::BITS == 64 { 64 } else { 32 };
    let idx = n / bits;
    if idx >= words.len() {
        return false;
    }
    let word = words[words.len() - 1 - idx];
    word >> (n % bits) & 1 == 1
}

fn held_in(bitmap: &[u8], buttons: &[u16]) -> bool {
    buttons
        .iter()
        .any(|b| bitmap[*b as usize / 8] >> (b % 8) & 1 == 1)
}

/// Mouse buttons of the Logitech mice, read from their evdev nodes on demand.
pub struct MouseButtons {
    sys: PathBuf,
    nodes: Mutex<(Vec<PathBuf>, Option<Instant>)>,
    pub denied: Mutex<bool>,
}

impl Default for MouseButtons {
    fn default() -> Self {
        Self::new(PathBuf::from("/sys/class/input"))
    }
}

impl MouseButtons {
    pub fn new(sys: PathBuf) -> Self {
        Self {
            sys,
            nodes: Mutex::new((Vec::new(), None)),
            denied: Mutex::new(false),
        }
    }

    /// Any button held on any Logitech mouse here; None when no node can be read.
    pub fn held(&self) -> Option<bool> {
        self.read(false, &[BTN_LEFT, BTN_RIGHT, BTN_MIDDLE])
    }

    /// The left button held on a Logitech mouse here, looking for its nodes again first (a
    /// mouse that just came back from another computer has a new one).
    pub fn left_held_now(&self) -> Option<bool> {
        self.read(true, &[BTN_LEFT])
    }

    fn read(&self, rescan: bool, buttons: &[u16]) -> Option<bool> {
        let nodes = {
            let mut n = lock(&self.nodes);
            if rescan || n.1.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
                *n = (logitech_mouse_nodes(&self.sys), Some(Instant::now()));
            }
            n.0.clone()
        };
        let mut known = false;
        for node in nodes {
            use std::os::unix::fs::OpenOptionsExt;
            let f = match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(&node)
            {
                Ok(f) => f,
                Err(e) => {
                    if e.kind() == io::ErrorKind::PermissionDenied {
                        *lock(&self.denied) = true;
                    }
                    continue;
                }
            };
            let mut bitmap = [0u8; KEY_MAX / 8 + 1];
            if ioctl_ptr(&f, eviocgkey(bitmap.len()), &mut bitmap).is_err() {
                continue;
            }
            known = true;
            if held_in(&bitmap, buttons) {
                return Some(true);
            }
        }
        known.then_some(false)
    }
}

// ================================================================ left button and motion
/// What the drag watch hears from the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEvent {
    /// The left button went down (true) or up (false).
    Left(bool),
    /// Relative motion in device units; only while [`PointerSource::want_motion`] is on.
    Motion(i32, i32),
}

/// Where the drag watch gets its events: the Logitech mouse ([`EvdevPointer`]) or a test.
pub trait PointerSource: Send {
    /// Events that arrived within `timeout` (empty when none came, or when woken).
    fn next(&mut self, timeout: Duration) -> io::Result<Vec<PointerEvent>>;
    /// Motion is wanted (from a left press until the drag is decided) or not.
    fn want_motion(&mut self, on: bool);
}

/// Wakes a thread blocked in [`EvdevPointer::next`] (an eventfd).
#[derive(Clone)]
pub struct Wake(Arc<OwnedFd>);

impl Wake {
    pub fn new() -> io::Result<Self> {
        use rustix::event::{EventfdFlags, eventfd};
        let fd = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
        Ok(Self(Arc::new(fd)))
    }

    pub fn wake(&self) {
        let _ = rustix::io::write(&*self.0, &1u64.to_ne_bytes());
    }

    fn clear(&self) {
        let mut b = [0u8; 8];
        let _ = rustix::io::read(&*self.0, &mut b);
    }
}

/// `struct input_event`: a timeval (two longs), u16 type, u16 code, s32 value.
const EVENT_SIZE: usize = 2 * std::mem::size_of::<libc::c_long>() + 8;
/// How often the Logitech nodes are looked for again (the mouse comes and goes with Flow).
const RESCAN: Duration = Duration::from_secs(3);

/// `struct input_mask` of `EVIOCSMASK`.
#[repr(C)]
struct InputMask {
    kind: u32,
    codes_size: u32,
    codes_ptr: u64,
}

/// `EVIOCSMASK`: of the `count` codes of event type `kind`, only `codes` reach this
/// process; `kind` 0 masks the event types themselves. `EV_SYN` is never masked, but a
/// report left empty by the mask is dropped, so masked events don't even wake us up.
fn set_mask(f: &File, kind: u16, count: usize, codes: &[u16]) -> io::Result<()> {
    let bits = libc::c_ulong::BITS as usize;
    let mut words: Vec<libc::c_ulong> = vec![0; count.div_ceil(bits)];
    for &c in codes {
        words[c as usize / bits] |= 1 << (c as usize % bits);
    }
    let mask = InputMask {
        kind: kind as u32,
        codes_size: std::mem::size_of_val(words.as_slice()) as u32,
        codes_ptr: words.as_ptr() as usize as u64,
    };
    // SAFETY: EVIOCSMASK reads one struct input_mask, whose pointer covers codes_size bytes.
    let r = unsafe { libc::ioctl(f.as_raw_fd(), EVIOCSMASK as _, &mask as *const InputMask) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// The event types that reach us: the left button always, X/Y motion only while wanted.
fn mask_types(f: &File, motion: bool) -> io::Result<()> {
    let types: &[u16] = if motion { &[EV_KEY, EV_REL] } else { &[EV_KEY] };
    set_mask(f, 0, EV_CNT, types)
}

fn left_held(f: &File) -> io::Result<bool> {
    let mut bitmap = [0u8; KEY_MAX / 8 + 1];
    ioctl_ptr(f, eviocgkey(bitmap.len()), &mut bitmap)?;
    Ok(bitmap[BTN_LEFT as usize / 8] >> (BTN_LEFT % 8) & 1 == 1)
}

struct Node {
    path: PathBuf,
    file: File,
    left: bool,
    /// `SYN_DROPPED`: what comes until the next `SYN_REPORT` is stale.
    dropped: bool,
    motion: (i32, i32),
}

impl Node {
    fn open(path: &Path, motion: bool) -> io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        // The filter goes first: the left button and X/Y motion, nothing else, ever.
        set_mask(&file, EV_KEY, KEY_CNT, &[BTN_LEFT])?;
        set_mask(&file, EV_REL, REL_CNT, &[REL_X, REL_Y])?;
        mask_types(&file, motion)?;
        // Whatever was queued in the instant before the filter: thrown away unparsed.
        let mut buf = [0u8; EVENT_SIZE * 64];
        while matches!(io::Read::read(&mut &file, &mut buf), Ok(n) if n > 0) {}
        let left = left_held(&file)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            left,
            dropped: false,
            motion: (0, 0),
        })
    }
}

/// The left button and (while wanted) the motion of the Logitech mice, read as events from
/// their evdev nodes ([`logitech_nodes`] with `pointer`: never a keyboard), filtered by
/// the kernel ([`set_mask`]). Never grabbed: the desktop gets every event as before.
pub struct EvdevPointer {
    sys: PathBuf,
    wake: Wake,
    nodes: Vec<Node>,
    scanned: Option<Instant>,
    motion: bool,
    /// The left button is held on one of the nodes (what was last reported).
    left: bool,
    /// A node couldn't be opened for lack of permission (the udev rule is missing).
    pub denied: bool,
}

impl EvdevPointer {
    pub fn new(sys: PathBuf, wake: Wake) -> Self {
        Self {
            sys,
            wake,
            nodes: Vec::new(),
            scanned: None,
            motion: false,
            left: false,
            denied: false,
        }
    }

    fn scan(&mut self) {
        self.scanned = Some(Instant::now());
        for path in logitech_nodes(&self.sys, true) {
            if self.nodes.iter().any(|n| n.path == path) {
                continue;
            }
            match Node::open(&path, self.motion) {
                Ok(n) => self.nodes.push(n),
                Err(e) => {
                    if e.kind() == io::ErrorKind::PermissionDenied {
                        self.denied = true;
                    }
                    log::debug!("Flow: {} not watched: {e}", path.display());
                }
            }
        }
    }

    /// A `Left` event when the state over every node changed.
    fn fold(&mut self, out: &mut Vec<PointerEvent>) {
        let any = self.nodes.iter().any(|n| n.left);
        if any != self.left {
            self.left = any;
            out.push(PointerEvent::Left(any));
        }
    }

    /// The events in `data` (whole `input_event`s) from node `i`.
    fn parse(&mut self, i: usize, data: &[u8], out: &mut Vec<PointerEvent>) {
        let off = 2 * std::mem::size_of::<libc::c_long>();
        for ev in data.chunks_exact(EVENT_SIZE) {
            let kind = u16::from_ne_bytes([ev[off], ev[off + 1]]);
            let code = u16::from_ne_bytes([ev[off + 2], ev[off + 3]]);
            let value = i32::from_ne_bytes([ev[off + 4], ev[off + 5], ev[off + 6], ev[off + 7]]);
            let n = &mut self.nodes[i];
            match (kind, code) {
                (EV_SYN, SYN_DROPPED) => n.dropped = true,
                (EV_SYN, SYN_REPORT) if n.dropped => {
                    n.dropped = false;
                    n.motion = (0, 0);
                    if let Ok(held) = left_held(&n.file) {
                        n.left = held;
                    }
                    self.fold(out);
                }
                (EV_SYN, SYN_REPORT) => {
                    let (dx, dy) = std::mem::take(&mut n.motion);
                    if self.motion && (dx, dy) != (0, 0) {
                        match out.last_mut() {
                            Some(PointerEvent::Motion(x, y)) => {
                                *x = x.saturating_add(dx);
                                *y = y.saturating_add(dy);
                            }
                            _ => out.push(PointerEvent::Motion(dx, dy)),
                        }
                    }
                }
                _ if n.dropped => {}
                (EV_KEY, BTN_LEFT) => {
                    n.left = value != 0;
                    self.fold(out);
                }
                (EV_REL, REL_X) => n.motion.0 = n.motion.0.saturating_add(value),
                (EV_REL, REL_Y) => n.motion.1 = n.motion.1.saturating_add(value),
                _ => {} // filtered by the kernel: never comes
            }
        }
    }
}

impl PointerSource for EvdevPointer {
    fn next(&mut self, timeout: Duration) -> io::Result<Vec<PointerEvent>> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let mut out = Vec::new();
        if self.scanned.is_none_or(|t| t.elapsed() >= RESCAN) {
            self.scan();
            self.fold(&mut out);
            if !out.is_empty() {
                return Ok(out);
            }
        }
        let to_scan = RESCAN.saturating_sub(self.scanned.map(|t| t.elapsed()).unwrap_or_default());
        let timeout = timeout.min(to_scan).max(Duration::from_millis(1));
        let revents: Vec<PollFlags> = {
            let mut fds: Vec<PollFd> = self
                .nodes
                .iter()
                .map(|n| PollFd::new(&n.file, PollFlags::IN))
                .collect();
            fds.push(PollFd::new(&*self.wake.0, PollFlags::IN));
            let ts = Timespec {
                tv_sec: timeout.as_secs() as _,
                tv_nsec: timeout.subsec_nanos() as _,
            };
            match poll(&mut fds, Some(&ts)) {
                Ok(_) => fds.iter().map(|f| f.revents()).collect(),
                Err(rustix::io::Errno::INTR) => return Ok(out),
                Err(e) => return Err(e.into()),
            }
        };
        if revents.last().is_some_and(|r| !r.is_empty()) {
            self.wake.clear();
        }
        let mut gone = Vec::new();
        let mut buf = [0u8; EVENT_SIZE * 64];
        for (i, r) in revents.iter().enumerate().take(self.nodes.len()) {
            if r.is_empty() {
                continue;
            }
            loop {
                match io::Read::read(&mut &self.nodes[i].file, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => self.parse(i, &buf[..n - n % EVENT_SIZE], &mut out),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        gone.push(i); // unplugged, or gone to the other computer
                        break;
                    }
                }
            }
            if r.intersects(PollFlags::HUP | PollFlags::ERR) && !gone.contains(&i) {
                gone.push(i);
            }
        }
        for i in gone.into_iter().rev() {
            self.nodes.remove(i);
        }
        self.fold(&mut out);
        Ok(out)
    }

    fn want_motion(&mut self, on: bool) {
        self.motion = on;
        let mut failed = Vec::new();
        for (i, n) in self.nodes.iter_mut().enumerate() {
            n.motion = (0, 0);
            if mask_types(&n.file, on).is_err() {
                failed.push(i);
            }
        }
        for i in failed.into_iter().rev() {
            self.nodes.remove(i); // can't be filtered any more: not read
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_bitmaps() {
        // a mouse: BTN_LEFT..BTN_TASK (0x110-0x117) in word 4 of 64-bit words
        let mouse = "ffff0000 0 0 0 0";
        let mouse64 = "1f0000 0 0 0 0";
        if usize::BITS == 64 {
            assert!(bit_set(mouse64, BTN_LEFT as usize));
            assert!(!bit_set("0", BTN_LEFT as usize));
        } else {
            assert!(!bit_set(mouse, 0));
        }
        let mut bm = [0u8; KEY_MAX / 8 + 1];
        let all = [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE];
        assert!(!held_in(&bm, &all));
        bm[BTN_RIGHT as usize / 8] |= 1 << (BTN_RIGHT % 8);
        assert!(held_in(&bm, &all));
        assert!(!held_in(&bm, &[BTN_LEFT]));
        assert_eq!(eviocgkey(96), 0x8060_4518);
    }

    #[test]
    fn finds_logitech_mouse_nodes() {
        let tmp = tempfile::tempdir().unwrap();
        let mk = |name: &str, vendor: &str, keys: &str| {
            let d = tmp.path().join(name).join("device");
            std::fs::create_dir_all(d.join("id")).unwrap();
            std::fs::create_dir_all(d.join("capabilities")).unwrap();
            std::fs::write(d.join("id").join("vendor"), vendor).unwrap();
            std::fs::write(d.join("capabilities").join("key"), keys).unwrap();
        };
        mk("event3", "046d\n", "1f0000 0 0 0 0\n");
        mk("event4", "046d\n", "0\n"); // a Logitech keyboard
        mk("event5", "1234\n", "1f0000 0 0 0 0\n");
        if usize::BITS == 64 {
            assert_eq!(
                logitech_mouse_nodes(tmp.path()),
                vec![PathBuf::from("/dev/input/event3")]
            );
        }
        // nodes that can't be opened: unknown
        assert_eq!(MouseButtons::new(tmp.path().join("none")).held(), None);
    }

    #[test]
    fn evdev_mask_ioctl() {
        // _IOW('E', 0x93, struct input_mask), as linux/input.h has it
        assert_eq!(std::mem::size_of::<InputMask>(), 16);
        let ioc = (1 << 30) | (16 << 16) | ((b'E' as libc::c_ulong) << 8) | 0x93;
        assert_eq!(EVIOCSMASK, ioc);
        assert_eq!(
            EVENT_SIZE,
            if cfg!(target_pointer_width = "64") {
                24
            } else {
                16
            }
        );
    }

    /// One `struct input_event`.
    fn ev(kind: u16, code: u16, value: i32) -> Vec<u8> {
        let mut b = vec![0u8; 2 * std::mem::size_of::<libc::c_long>()];
        b.extend(kind.to_ne_bytes());
        b.extend(code.to_ne_bytes());
        b.extend(value.to_ne_bytes());
        b
    }

    #[test]
    fn evdev_left_button_and_motion() {
        use std::io::Write;
        let (rd, wr) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::NONBLOCK).unwrap();
        let mut wr = File::from(wr);
        let tmp = tempfile::tempdir().unwrap();
        let mut p = EvdevPointer::new(tmp.path().to_path_buf(), Wake::new().unwrap());
        p.scanned = Some(Instant::now());
        p.nodes.push(Node {
            path: PathBuf::from("/dev/input/event-test"),
            file: File::from(rd),
            left: false,
            dropped: false,
            motion: (0, 0),
        });
        let syn = || ev(EV_SYN, SYN_REPORT, 0);
        let next = |p: &mut EvdevPointer, wr: &mut File, data: Vec<Vec<u8>>| {
            wr.write_all(&data.concat()).unwrap();
            p.next(Duration::from_millis(200)).unwrap()
        };
        // motion while not asked for: nothing (the kernel wouldn't even send it)
        assert!(next(&mut p, &mut wr, vec![ev(EV_REL, REL_X, 5), syn()]).is_empty());
        assert_eq!(
            next(&mut p, &mut wr, vec![ev(EV_KEY, BTN_LEFT, 1), syn()]),
            [PointerEvent::Left(true)]
        );
        p.motion = true; // (want_motion would set the kernel's filter)
        assert_eq!(
            next(
                &mut p,
                &mut wr,
                vec![
                    ev(EV_REL, REL_X, 5),
                    ev(EV_REL, REL_Y, -3),
                    syn(),
                    ev(EV_REL, REL_X, 2),
                    syn(),
                ]
            ),
            [PointerEvent::Motion(7, -3)]
        );
        // events lost: the rest of that report is stale, the state is read again
        assert!(
            next(
                &mut p,
                &mut wr,
                vec![
                    ev(EV_SYN, SYN_DROPPED, 0),
                    ev(EV_KEY, BTN_LEFT, 0),
                    ev(EV_REL, REL_X, 50),
                    syn(),
                ]
            )
            .is_empty()
        );
        assert_eq!(
            next(&mut p, &mut wr, vec![ev(EV_KEY, BTN_LEFT, 0), syn()]),
            [PointerEvent::Left(false)]
        );
        // the mouse goes to the other computer with the button held: let go
        assert_eq!(
            next(&mut p, &mut wr, vec![ev(EV_KEY, BTN_LEFT, 1), syn()]),
            [PointerEvent::Left(true)]
        );
        drop(wr);
        assert_eq!(
            p.next(Duration::from_millis(200)).unwrap(),
            [PointerEvent::Left(false)]
        );
        assert!(p.nodes.is_empty());
        // woken up at once when the watch stops
        let wake = p.wake.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            wake.wake();
        });
        let t0 = Instant::now();
        assert!(p.next(Duration::from_secs(10)).unwrap().is_empty());
        assert!(t0.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn only_mice_are_watched() {
        let tmp = tempfile::tempdir().unwrap();
        let mk = |name: &str, keys: &str, rel: &str| {
            let d = tmp.path().join(name).join("device");
            std::fs::create_dir_all(d.join("id")).unwrap();
            std::fs::create_dir_all(d.join("capabilities")).unwrap();
            std::fs::write(d.join("id").join("vendor"), "046d\n").unwrap();
            std::fs::write(d.join("capabilities").join("key"), keys).unwrap();
            std::fs::write(d.join("capabilities").join("rel"), rel).unwrap();
        };
        mk("event3", "1f0000 0 0 0 0\n", "1c3\n"); // a mouse: buttons, X/Y, wheels
        // a keyboard with a touchpad on one node (letter keys too)
        mk("event4", "1f0000 0 0 0 fffffffffffffffe\n", "3\n");
        mk("event5", "1f0000 0 0 0 0\n", "0\n"); // buttons without motion
        if usize::BITS == 64 {
            assert_eq!(
                logitech_nodes(tmp.path(), true),
                vec![PathBuf::from("/dev/input/event3")]
            );
            assert_eq!(logitech_nodes(tmp.path(), false).len(), 3);
        }
    }
}
