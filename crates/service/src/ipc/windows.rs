//! The named-pipe server (`\\.\pipe\glisa-<SID>`): DACL for the current user only,
//! remote clients rejected, first instance exclusive. Also the helpers the client uses
//! to check that a pipe belongs to this user.

use std::io::{self, Read, Write};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ,
    GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_SHARE_NONE, FlushFileBuffers, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::service::Service;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

/// The current user's token information (TOKEN_USER), kept alive in a buffer.
struct UserToken {
    buf: Vec<u64>,
}

impl UserToken {
    fn current() -> io::Result<Self> {
        let mut token: HANDLE = null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo-handle; token is valid for writes.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(last_error());
        }
        let mut len = 0u32;
        // SAFETY: size query with a null buffer.
        unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len) };
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        // SAFETY: buf has at least len bytes; token is open.
        let ok = unsafe {
            GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
        };
        // SAFETY: token was opened above and is closed once.
        unsafe { CloseHandle(token) };
        if ok == 0 {
            return Err(last_error());
        }
        Ok(UserToken { buf })
    }

    fn sid(&self) -> PSID {
        // SAFETY: buf holds a TOKEN_USER written by GetTokenInformation.
        unsafe { (*(self.buf.as_ptr() as *const TOKEN_USER)).User.Sid }
    }
}

/// "S-1-5-21-…" of the current user.
pub fn current_user_sid() -> io::Result<String> {
    let token = UserToken::current()?;
    let mut text: *mut u16 = null_mut();
    // SAFETY: the SID is valid while token lives; text receives a LocalAlloc'ed string.
    if unsafe { ConvertSidToStringSidW(token.sid(), &mut text) } == 0 {
        return Err(last_error());
    }
    // SAFETY: text is a NUL-terminated UTF-16 string from the call above.
    let s = unsafe {
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(text, n))
    };
    // SAFETY: text was allocated by ConvertSidToStringSidW.
    unsafe { LocalFree(text.cast()) };
    Ok(s)
}

/// Whether the pipe `handle` is owned by the current user.
pub fn owned_by_current_user(handle: HANDLE) -> bool {
    let Ok(token) = UserToken::current() else {
        return false;
    };
    let mut owner: PSID = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: handle is an open pipe; the out pointers are valid for writes.
    let r = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        )
    };
    if r != 0 || owner.is_null() {
        return false;
    }
    // SAFETY: both SIDs are valid; sd was allocated by GetSecurityInfo.
    let same = unsafe { EqualSid(owner, token.sid()) } != 0;
    unsafe { LocalFree(sd) };
    same
}

/// An open pipe end (server instance or client), closed on drop.
pub struct Pipe {
    handle: HANDLE,
    server: bool,
}

// SAFETY: a pipe HANDLE may be used from any thread; each Pipe is used by one thread.
unsafe impl Send for Pipe {}
// SAFETY: reads and writes on one synchronous handle are serialised by the system.
unsafe impl Sync for Pipe {}

impl Pipe {
    /// Connects to `name` as a client, waiting up to `timeout_ms` for a free instance.
    pub fn connect(name: &str, timeout_ms: u32) -> io::Result<Pipe> {
        let w = wide(name);
        loop {
            // SAFETY: w is a NUL-terminated path; no security attributes or template.
            let h = unsafe {
                CreateFileW(
                    w.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_NONE,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            if h != INVALID_HANDLE_VALUE {
                return Ok(Pipe {
                    handle: h,
                    server: false,
                });
            }
            // SAFETY: plain query of the calling thread's last error.
            if unsafe { GetLastError() } != ERROR_PIPE_BUSY {
                return Err(last_error());
            }
            // SAFETY: w is a NUL-terminated pipe name.
            if unsafe { WaitNamedPipeW(w.as_ptr(), timeout_ms) } == 0 {
                return Err(last_error());
            }
        }
    }

    pub fn handle(&self) -> HANDLE {
        self.handle
    }
}

impl Read for &Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0u32;
        let len = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: buf is valid for len bytes; synchronous I/O (no OVERLAPPED).
        if unsafe { ReadFile(self.handle, buf.as_mut_ptr(), len, &mut n, null_mut()) } == 0 {
            // SAFETY: plain query of the last error.
            if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
                return Ok(0);
            }
            return Err(last_error());
        }
        Ok(n as usize)
    }
}

impl Write for &Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut n = 0u32;
        let len = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: buf is valid for len bytes; synchronous I/O.
        if unsafe { WriteFile(self.handle, buf.as_ptr(), len, &mut n, null_mut()) } == 0 {
            return Err(last_error());
        }
        Ok(n as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: the handle is open and closed exactly once here.
        unsafe {
            if self.server {
                FlushFileBuffers(self.handle);
                DisconnectNamedPipe(self.handle);
            }
            CloseHandle(self.handle);
        }
    }
}

/// A security descriptor granting the current user everything and nobody else
/// anything ("D:P(A;;GA;;;<SID>)", protected from inheritance).
struct OwnerOnly {
    sd: PSECURITY_DESCRIPTOR,
}

impl OwnerOnly {
    fn new() -> io::Result<Self> {
        let sddl = wide(&format!("D:P(A;;GA;;;{})", current_user_sid()?));
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: sddl is NUL-terminated; sd receives a LocalAlloc'ed descriptor.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error());
        }
        Ok(OwnerOnly { sd })
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        // SAFETY: sd was allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.sd) };
    }
}

fn create_instance(name: &[u16], first: bool, sd: &OwnerOnly) -> io::Result<Pipe> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.sd,
        bInheritHandle: 0,
    };
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: name is NUL-terminated; sa and its descriptor outlive the call.
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            &sa,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    Ok(Pipe {
        handle: h,
        server: true,
    })
}

pub struct PipeServer {
    name: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PipeServer {
    pub fn start(service: Weak<Service>, name: &str) -> io::Result<PipeServer> {
        let sd = OwnerOnly::new()?;
        let wname = wide(name);
        // Fails if anybody (another user included) already created this pipe.
        let first = create_instance(&wname, true, &sd).map_err(|e| {
            io::Error::new(
                io::ErrorKind::AddrInUse,
                glisa_i18n::tr!(
                    "A Glisa service is already listening on {path}",
                    path = name
                ) + &format!(" ({e})"),
            )
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name(format!("{}-ipc", glisa_common::APP_DIR))
            .spawn(move || accept_loop(first, wname, sd, service, stop2))?;
        Ok(PipeServer {
            name: name.to_string(),
            stop,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = Pipe::connect(&self.name, 500); // wakes ConnectNamedPipe
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        self.stop();
    }
}

// SAFETY: the descriptor is only read (by CreateNamedPipeW) from the accept thread.
unsafe impl Send for OwnerOnly {}

fn accept_loop(
    first: Pipe,
    name: Vec<u16>,
    sd: OwnerOnly,
    service: Weak<Service>,
    stop: Arc<AtomicBool>,
) {
    let mut next = Some(first);
    loop {
        let pipe = match next.take() {
            Some(p) => p,
            None => match create_instance(&name, false, &sd) {
                Ok(p) => p,
                Err(e) => {
                    log::error!("IPC: cannot create a pipe instance: {e}");
                    return;
                }
            },
        };
        // SAFETY: the pipe handle is open; synchronous connect.
        let ok = unsafe { ConnectNamedPipe(pipe.handle, null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if !ok {
            continue;
        }
        let service = service.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("{}-ipc-conn", glisa_common::APP_DIR))
            .spawn(move || super::serve_connection(service, &pipe));
        if spawned.is_err() {
            log::warn!("could not start a thread for an IPC client");
        }
    }
}
