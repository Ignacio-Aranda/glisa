//! The Unix socket server, and the peer-uid check shared with the client.

use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;

use glisa_i18n::tr;

use crate::service::Service;

/// This process's uid.
pub fn my_uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    unsafe { libc::getuid() }
}

/// The uid of the process at the other end of a connected Unix socket.
pub fn peer_uid(sock: &UnixStream) -> Option<u32> {
    let fd = sock.as_raw_fd();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: u32::MAX,
            gid: u32::MAX,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: cred and len are valid for writes of the sizes given.
        let r = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        (r == 0 && len as usize == std::mem::size_of::<libc::ucred>()).then_some(cred.uid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let mut uid: libc::uid_t = u32::MAX;
        let mut gid: libc::gid_t = u32::MAX;
        // SAFETY: uid and gid are valid for writes.
        let r = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
        (r == 0).then_some(uid)
    }
}

pub struct UnixServer {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl UnixServer {
    /// Binds `path` (0600) and serves clients of `allowed_uid` (default: this user).
    pub fn bind(
        service: Weak<Service>,
        path: &str,
        allowed_uid: Option<u32>,
    ) -> io::Result<UnixServer> {
        let path = PathBuf::from(path);
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if UnixStream::connect(&path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    tr!(
                        "A Glisa service is already listening on {path}",
                        path = path.display()
                    ),
                ));
            }
            if !meta.file_type().is_socket() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a socket", path.display()),
                ));
            }
            std::fs::remove_file(&path)?; // a stale socket of ours
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let allowed = allowed_uid.unwrap_or_else(my_uid);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name(format!("{}-ipc", glisa_common::APP_DIR))
            .spawn(move || accept_loop(listener, service, stop2, allowed))?;
        Ok(UnixServer {
            path,
            stop,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = UnixStream::connect(&self.path); // wakes accept()
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.file_type().is_socket()) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Drop for UnixServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(
    listener: UnixListener,
    service: Weak<Service>,
    stop: Arc<AtomicBool>,
    allowed: u32,
) {
    for conn in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(mut stream) = conn else { continue };
        let service = service.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("{}-ipc-conn", glisa_common::APP_DIR))
            .spawn(move || {
                let uid = peer_uid(&stream);
                if uid != Some(allowed) {
                    // The socket is 0600 already; this also refuses root's or other
                    // users' clients that could still reach it.
                    log::warn!(
                        "connection to the service refused: it belongs to another user (uid {uid:?})"
                    );
                    let _ = stream.write_all(&super::access_denied());
                    return;
                }
                super::serve_connection(service, &stream);
            });
        if spawned.is_err() {
            log::warn!("could not start a thread for an IPC client");
        }
    }
}
