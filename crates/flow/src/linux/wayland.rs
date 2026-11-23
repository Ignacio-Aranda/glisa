//! A tiny Wayland wire-protocol client: just enough for two small read/write protocols
//! (`org_kde_kwin_keystate` and the data-control clipboard), written against the wire
//! format directly like the Python version (no libwayland, no generated bindings).
//!
//! Message: object id (u32) ‖ size << 16 | opcode (u32) ‖ arguments, native endian;
//! strings are a u32 length (with the NUL) + bytes + NUL + padding to 4; file descriptors
//! travel as SCM_RIGHTS ancillary data.

use std::collections::VecDeque;
use std::io::{self, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};

pub const DISPLAY: u32 = 1;
/// Ids the compositor creates start here.
pub const SERVER_IDS: u32 = 0xFF00_0000;

/// The compositor's socket: `$WAYLAND_DISPLAY` (absolute, or in `$XDG_RUNTIME_DIR`).
pub fn socket_path() -> Option<PathBuf> {
    let name = std::env::var_os("WAYLAND_DISPLAY").filter(|v| !v.is_empty())?;
    let name = PathBuf::from(name);
    if name.is_absolute() {
        return Some(name);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(runtime).join(name))
}

pub fn connect(timeout: Duration) -> io::Result<UnixStream> {
    let path = socket_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Wayland session"))?;
    let s = UnixStream::connect(path)?;
    s.set_write_timeout(Some(timeout))?;
    Ok(s)
}

/// A string argument.
pub fn string(text: &str) -> Vec<u8> {
    let mut b = text.as_bytes().to_vec();
    b.push(0);
    let mut out = (b.len() as u32).to_ne_bytes().to_vec();
    let pad = (4 - b.len() % 4) % 4;
    out.extend(b);
    out.extend(std::iter::repeat_n(0u8, pad));
    out
}

pub fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

pub fn read_u32(body: &[u8], off: usize) -> io::Result<u32> {
    body.get(off..off + 4)
        .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| bad("short Wayland message"))
}

/// (string, offset after it).
pub fn read_string(body: &[u8], off: usize) -> io::Result<(String, usize)> {
    let n = read_u32(body, off)? as usize;
    let end = off + 4 + n;
    if n == 0 || end > body.len() {
        return Err(bad("bad Wayland string"));
    }
    let s = String::from_utf8_lossy(&body[off + 4..end - 1]).into_owned();
    Ok((s, end + (4 - n % 4) % 4))
}

pub fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// One message from the compositor.
#[derive(Debug, Clone)]
pub struct Message {
    pub object: u32,
    pub opcode: u16,
    pub body: Vec<u8>,
}

/// The connection: writes from any thread (`send_lock`), reads from one thread.
pub struct Wire {
    pub sock: UnixStream,
    send_lock: Mutex<()>,
    buf: Vec<u8>,
    pub fds: VecDeque<OwnedFd>,
}

impl Wire {
    pub fn new(sock: UnixStream) -> Self {
        Self {
            sock,
            send_lock: Mutex::new(()),
            buf: Vec::new(),
            fds: VecDeque::new(),
        }
    }

    pub fn send(&self, object: u32, opcode: u16, payload: &[u8]) -> io::Result<()> {
        send_on(&self.sock, &self.send_lock, object, opcode, payload, None)
    }

    pub fn send_fd(
        &self,
        object: u32,
        opcode: u16,
        payload: &[u8],
        fd: BorrowedFd<'_>,
    ) -> io::Result<()> {
        send_on(
            &self.sock,
            &self.send_lock,
            object,
            opcode,
            payload,
            Some(fd),
        )
    }

    /// A handle other threads send requests with (the reading stays here).
    pub fn sender(&self) -> io::Result<std::sync::Arc<Sender>> {
        Ok(std::sync::Arc::new(Sender {
            sock: self.sock.try_clone()?,
            lock: Mutex::new(()),
        }))
    }

    /// Messages that arrived within `timeout` (empty if none).
    pub fn read(&mut self, timeout: Duration) -> io::Result<Vec<Message>> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let ready = {
            let fd = self.sock.as_fd();
            let mut fds = [PollFd::new(&fd, PollFlags::IN)];
            let ts = Timespec {
                tv_sec: timeout.as_secs() as _,
                tv_nsec: timeout.subsec_nanos() as _,
            };
            poll(&mut fds, Some(&ts))? > 0
        };
        if ready {
            let mut data = vec![0u8; 65536];
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(28))];
            let mut cmsg = RecvAncillaryBuffer::new(&mut space);
            let r = rustix::net::recvmsg(
                &self.sock,
                &mut [IoSliceMut::new(&mut data)],
                &mut cmsg,
                RecvFlags::CMSG_CLOEXEC,
            )?;
            for m in cmsg.drain() {
                if let RecvAncillaryMessage::ScmRights(fds) = m {
                    self.fds.extend(fds);
                }
            }
            if r.bytes == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the compositor closed the connection",
                ));
            }
            self.buf.extend_from_slice(&data[..r.bytes]);
        }
        let mut out = Vec::new();
        while self.buf.len() >= 8 {
            let object = read_u32(&self.buf, 0)?;
            let size_op = read_u32(&self.buf, 4)?;
            let size = (size_op >> 16) as usize;
            if size < 8 {
                return Err(bad("invalid Wayland message"));
            }
            if self.buf.len() < size {
                break;
            }
            let body = self.buf[8..size].to_vec();
            self.buf.drain(..size);
            out.push(Message {
                object,
                opcode: (size_op & 0xFFFF) as u16,
                body,
            });
        }
        Ok(out)
    }
}

/// Sends requests from any thread.
pub struct Sender {
    pub sock: UnixStream,
    lock: Mutex<()>,
}

impl Sender {
    pub fn send(&self, object: u32, opcode: u16, payload: &[u8]) -> io::Result<()> {
        send_on(&self.sock, &self.lock, object, opcode, payload, None)
    }

    pub fn send_fd(
        &self,
        object: u32,
        opcode: u16,
        payload: &[u8],
        fd: BorrowedFd<'_>,
    ) -> io::Result<()> {
        send_on(&self.sock, &self.lock, object, opcode, payload, Some(fd))
    }

    /// Ends the connection (wakes the reading thread).
    pub fn shutdown(&self) {
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
    }
}

/// Sends one request on `sock` (with a file descriptor when given).
pub fn send_on(
    sock: &UnixStream,
    lock: &Mutex<()>,
    object: u32,
    opcode: u16,
    payload: &[u8],
    fd: Option<BorrowedFd<'_>>,
) -> io::Result<()> {
    let mut data = Vec::with_capacity(8 + payload.len());
    data.extend_from_slice(&object.to_ne_bytes());
    data.extend_from_slice(&((((8 + payload.len()) as u32) << 16) | opcode as u32).to_ne_bytes());
    data.extend_from_slice(payload);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut cmsg = SendAncillaryBuffer::new(&mut space);
    let fds;
    if let Some(fd) = fd {
        fds = [fd];
        cmsg.push(SendAncillaryMessage::ScmRights(&fds));
    }
    let mut sent =
        rustix::net::sendmsg(sock, &[IoSlice::new(&data)], &mut cmsg, SendFlags::NOSIGNAL)?;
    while sent < data.len() {
        let mut nospace: [MaybeUninit<u8>; 0] = [];
        let mut empty = SendAncillaryBuffer::new(&mut nospace);
        sent += rustix::net::sendmsg(
            sock,
            &[IoSlice::new(&data[sent..])],
            &mut empty,
            SendFlags::NOSIGNAL,
        )?;
    }
    Ok(())
}

/// Globals advertised by the registry: interface → (name, version).
pub type Globals = std::collections::HashMap<String, (u32, u32)>;

/// get_registry (id `registry`) + sync (id `sync`), then the globals until the sync.
pub fn globals(
    wire: &mut Wire,
    registry: u32,
    sync: u32,
    timeout: Duration,
) -> io::Result<Globals> {
    wire.send(DISPLAY, 1, &u32s(&[registry]))?; // get_registry
    wire.send(DISPLAY, 0, &u32s(&[sync]))?; // sync
    let mut globals = Globals::new();
    let end = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < end {
        for m in wire.read(Duration::from_millis(50))? {
            if m.object == registry && m.opcode == 0 {
                let name = read_u32(&m.body, 0)?;
                let (iface, off) = read_string(&m.body, 4)?;
                let version = read_u32(&m.body, off)?;
                globals.entry(iface).or_insert((name, version));
            } else if m.object == sync {
                return Ok(globals);
            } else if m.object == DISPLAY && m.opcode == 0 {
                return Err(bad("Wayland protocol error"));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "the compositor didn't answer",
    ))
}

/// registry.bind(name, interface, version, new id).
pub fn bind(
    wire: &Wire,
    registry: u32,
    name: u32,
    iface: &str,
    version: u32,
    id: u32,
) -> io::Result<()> {
    let mut p = u32s(&[name]);
    p.extend(string(iface));
    p.extend(u32s(&[version, id]));
    wire.send(registry, 0, &p)
}
