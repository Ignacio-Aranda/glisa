//! Small helpers shared by the Flow modules: Python-compatible character classes, hashing,
//! random bytes, a lock that can be waited for with a timeout, and "wait until readable".

use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use unicode_general_category::{GeneralCategory, get_general_category};

/// Python's `str.isprintable()` for one character: false for the Unicode categories
/// "Other" (Cc, Cf, Cs, Co, Cn) and "Separator" (Zl, Zp, Zs), except the ASCII space.
pub fn is_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    !matches!(
        get_general_category(c),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
            | GeneralCategory::SpaceSeparator
    )
}

/// Python's `str.isprintable()`.
pub fn printable(s: &str) -> bool {
    s.chars().all(is_printable)
}

/// Characters a relative path in a transfer may never contain: control, format (bidi
/// overrides…), surrogate, private-use and unassigned characters, line and paragraph
/// separators (Python `transfer` categories Cc Cf Cs Co Cn Zl Zp).
pub fn forbidden_in_path(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
    )
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn is_hex_lower(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, data);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&sha256(data))
}

/// Incremental SHA-256.
pub struct Sha256(ring::digest::Context);

impl Sha256 {
    pub fn new() -> Self {
        Self(ring::digest::Context::new(&ring::digest::SHA256))
    }
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    pub fn hex(self) -> String {
        hex(self.0.finish().as_ref())
    }
}

/// Cryptographically secure random bytes (the OS generator).
pub fn random_bytes<const N: usize>() -> [u8; N] {
    use ring::rand::SecureRandom;
    let mut out = [0u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut out)
        .expect("the system random number generator failed");
    out
}

pub fn random_hex(bytes: usize) -> String {
    let mut out = String::new();
    while out.len() < bytes * 2 {
        out.push_str(&hex(&random_bytes::<16>()));
    }
    out.truncate(bytes * 2);
    out
}

/// Seconds since the Unix epoch, as Python's `time.time()`.
pub fn unix_time() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// A mutex that can be waited for with a timeout (Python's `Lock.acquire(timeout=…)`).
pub struct TimedMutex<T> {
    busy: Mutex<bool>,
    cv: Condvar,
    data: Mutex<T>,
}

pub struct TimedGuard<'a, T> {
    owner: &'a TimedMutex<T>,
    guard: Option<MutexGuard<'a, T>>,
}

impl<T> TimedMutex<T> {
    pub fn new(value: T) -> Self {
        Self {
            busy: Mutex::new(false),
            cv: Condvar::new(),
            data: Mutex::new(value),
        }
    }

    fn take(&self) -> TimedGuard<'_, T> {
        TimedGuard {
            owner: self,
            guard: Some(self.data.lock().unwrap_or_else(|e| e.into_inner())),
        }
    }

    pub fn lock(&self) -> TimedGuard<'_, T> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        while *busy {
            busy = self.cv.wait(busy).unwrap_or_else(|e| e.into_inner());
        }
        *busy = true;
        drop(busy);
        self.take()
    }

    pub fn try_lock(&self) -> Option<TimedGuard<'_, T>> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        if *busy {
            return None;
        }
        *busy = true;
        drop(busy);
        Some(self.take())
    }

    pub fn lock_for(&self, timeout: Duration) -> Option<TimedGuard<'_, T>> {
        let deadline = Instant::now() + timeout;
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        while *busy {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            busy = self
                .cv
                .wait_timeout(busy, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *busy = true;
        drop(busy);
        Some(self.take())
    }
}

impl<T> std::ops::Deref for TimedGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_ref().expect("guard")
    }
}

impl<T> std::ops::DerefMut for TimedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().expect("guard")
    }
}

impl<T> Drop for TimedGuard<'_, T> {
    fn drop(&mut self) {
        self.guard = None;
        let mut busy = self.owner.busy.lock().unwrap_or_else(|e| e.into_inner());
        *busy = false;
        self.owner.cv.notify_one();
    }
}

/// Locks a std mutex, ignoring poisoning (a panicking thread must not stop Flow).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Waits until a listening socket has a connection to accept (or `timeout` passes).
#[cfg(unix)]
pub fn wait_readable(sock: &std::net::TcpListener, timeout: Duration) -> bool {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use std::os::fd::AsFd;
    let fd = sock.as_fd();
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    let ts = Timespec {
        tv_sec: timeout.as_secs() as _,
        tv_nsec: timeout.subsec_nanos() as _,
    };
    matches!(poll(&mut fds, Some(&ts)), Ok(n) if n > 0)
}

#[cfg(windows)]
pub fn wait_readable(sock: &std::net::TcpListener, timeout: Duration) -> bool {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{POLLRDNORM, WSAPOLLFD, WSAPoll};
    let mut fd = WSAPOLLFD {
        fd: sock.as_raw_socket() as usize,
        events: POLLRDNORM,
        revents: 0,
    };
    // SAFETY: one valid WSAPOLLFD for the duration of the call.
    let n = unsafe { WSAPoll(&mut fd, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
    n > 0
}

#[cfg(not(any(unix, windows)))]
pub fn wait_readable(_sock: &std::net::TcpListener, timeout: Duration) -> bool {
    std::thread::sleep(timeout);
    true
}

/// Name of this computer (`socket.gethostname()`).
pub fn hostname() -> String {
    #[cfg(unix)]
    {
        rustix::system::uname()
            .nodename()
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(windows)]
    {
        crate::windows::computer_name()
    }
    #[cfg(not(any(unix, windows)))]
    {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_like_python() {
        assert!(printable("portátil-ana 2"));
        assert!(!printable("a\nb"));
        assert!(!printable("a\u{202e}b"));
        assert!(!printable("a\u{a0}b"));
        assert!(!printable("\u{e000}"));
        assert!(!printable("\u{378}")); // unassigned
        assert!(!printable("👩‍💻")); // ZWJ is a format character
        assert!(printable("日本"));
    }

    #[test]
    fn timed_mutex() {
        let m = TimedMutex::new(1);
        let g = m.lock();
        assert!(m.try_lock().is_none());
        assert!(m.lock_for(Duration::from_millis(20)).is_none());
        drop(g);
        assert_eq!(*m.lock_for(Duration::from_millis(20)).unwrap(), 1);
        assert_eq!(unhex("0aff"), Some(vec![10, 255]));
    }
}
