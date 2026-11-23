//! Local IPC for `glisa-ctl`: JSON lines, one request per line,
//! `{"id", "method", "params", "lang"}` → `{"id", "result"}` or `{"id", "error"}`.
//!
//! - Linux/macOS: a Unix socket, mode 0600, in the private runtime directory
//!   (`common::paths::runtime_dir`), and every client's uid is checked
//!   (`SO_PEERCRED` / `getpeereid`): other users are refused.
//! - Windows: the named pipe `\\.\pipe\glisa-<user SID>`, whose DACL grants access
//!   to the current user only, with `PIPE_REJECT_REMOTE_CLIENTS`; the first instance is
//!   created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a pipe squatted by someone else
//!   makes the service fail instead of sharing the name.
//!
//! Lines are limited to [`MAX_LINE`] bytes; anything longer closes the connection.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::Weak;

use glisa_i18n::tr;
use serde_json::{Value, json};

use crate::service::Service;

#[cfg(unix)]
pub(crate) mod unix;
#[cfg(windows)]
pub(crate) mod windows;

/// Longest request line (Flow's clipboard fallback carries text and up to 10 000 URIs).
pub const MAX_LINE: usize = 16 << 20;

/// The endpoint the service listens on by default.
pub fn default_endpoint() -> io::Result<String> {
    let base = glisa_common::paths::ipc_endpoint()?;
    #[cfg(windows)]
    {
        Ok(format!("{base}-{}", windows::current_user_sid()?))
    }
    #[cfg(not(windows))]
    {
        Ok(base)
    }
}

/// A running IPC server.
pub struct Server {
    #[cfg(unix)]
    inner: unix::UnixServer,
    #[cfg(windows)]
    inner: windows::PipeServer,
}

impl Server {
    pub fn stop(&mut self) {
        #[cfg(any(unix, windows))]
        self.inner.stop();
    }
}

/// Listens on `endpoint` until [`Server::stop`].
pub fn serve(service: Weak<Service>, endpoint: &str) -> io::Result<Server> {
    #[cfg(unix)]
    {
        Ok(Server {
            inner: unix::UnixServer::bind(service, endpoint, None)?,
        })
    }
    #[cfg(windows)]
    {
        Ok(Server {
            inner: windows::PipeServer::start(service, endpoint)?,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (service, endpoint);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no IPC on this OS",
        ))
    }
}

/// Like [`serve`], accepting only clients running as `uid` (tests simulate another
/// user with it).
#[cfg(unix)]
#[doc(hidden)]
pub fn serve_for_uid(service: Weak<Service>, endpoint: &str, uid: u32) -> io::Result<Server> {
    Ok(Server {
        inner: unix::UnixServer::bind(service, endpoint, Some(uid))?,
    })
}

/// One line (without the newline); `None` at end of stream. Longer than `max`: error.
pub fn read_line_limited<R: BufRead>(r: &mut R, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok(if out.is_empty() { None } else { Some(out) });
        }
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            out.extend_from_slice(&buf[..i]);
            r.consume(i + 1);
            if out.len() > max {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
            }
            return Ok(Some(out));
        }
        let n = buf.len();
        out.extend_from_slice(buf);
        r.consume(n);
        if out.len() > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
    }
}

/// The answer line (with its newline) to one request line.
pub(crate) fn answer(service: &Service, line: &[u8]) -> Vec<u8> {
    let resp = match serde_json::from_slice::<Value>(line) {
        Ok(Value::Object(req)) => {
            let id = req.get("id").cloned().unwrap_or(Value::Null);
            let method = req.get("method").and_then(Value::as_str).unwrap_or("");
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let lang = req.get("lang").and_then(Value::as_str);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                service.handle(method, &params, lang)
            }));
            match r {
                Ok(Ok(v)) => json!({"id": id, "result": v}),
                Ok(Err(e)) => json!({"id": id, "error": e.0}),
                Err(_) => {
                    log::error!("request {method} panicked");
                    json!({"id": id, "error": "internal error"})
                }
            }
        }
        _ => json!({"id": null, "error": tr!("Invalid request")}),
    };
    let mut out = serde_json::to_vec(&resp).unwrap_or_default();
    out.push(b'\n');
    out
}

/// Serves one connection until it closes.
pub(crate) fn serve_connection<S>(service: Weak<Service>, stream: &S)
where
    for<'a> &'a S: Read + Write,
{
    let mut reader = BufReader::new(stream);
    let mut writer = stream;
    loop {
        let line = match read_line_limited(&mut reader, MAX_LINE) {
            Ok(Some(l)) => l,
            Ok(None) | Err(_) => return,
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Some(s) = service.upgrade() else { return };
        let out = answer(&s, &line);
        drop(s);
        if writer
            .write_all(&out)
            .and_then(|()| writer.flush())
            .is_err()
        {
            return;
        }
    }
}

/// The refusal sent to a client of another user.
#[cfg(unix)]
pub(crate) fn access_denied() -> Vec<u8> {
    let mut out =
        serde_json::to_vec(&json!({"id": null, "error": tr!("Access denied")})).unwrap_or_default();
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_limits() {
        let mut r = BufReader::with_capacity(4, &b"abc\ndefghij\nxy"[..]);
        assert_eq!(
            read_line_limited(&mut r, 10).unwrap(),
            Some(b"abc".to_vec())
        );
        assert_eq!(
            read_line_limited(&mut r, 10).unwrap(),
            Some(b"defghij".to_vec())
        );
        assert_eq!(read_line_limited(&mut r, 10).unwrap(), Some(b"xy".to_vec()));
        assert_eq!(read_line_limited(&mut r, 10).unwrap(), None);
        let mut r = BufReader::with_capacity(4, &b"0123456789abc\n"[..]);
        assert!(read_line_limited(&mut r, 10).is_err());
    }
}
