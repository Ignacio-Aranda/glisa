//! Client of the service's IPC API (used by `glisa-ctl`).
//!
//! Requests carry clipboard text and commands and answers can set the clipboard, so
//! the client only talks to a service of this same user: on Unix the socket's peer uid
//! must be ours, on Windows the pipe must be owned by our SID. Answers come in the
//! client's language (each request carries it).

use std::fmt;
use std::io::{BufReader, Write};
use std::time::Duration;

use glisa_i18n::tr;
use serde_json::{Value, json};

use crate::fsutil::error_text;
use crate::ipc::{MAX_LINE, read_line_limited};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// Could not reach a trustworthy service (message for people).
    Connect(String),
    /// The service answered with an error (already in our language).
    Remote(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Connect(m) | ClientError::Remote(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ClientError {}

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = crate::ipc::windows::Pipe;

struct Conn {
    stream: std::sync::Arc<Stream>,
    reader: BufReader<ArcRead>,
}

/// Reads through a shared stream (the writer uses the same one).
struct ArcRead(std::sync::Arc<Stream>);

impl std::io::Read for ArcRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (&*self.0).read(buf)
    }
}

pub struct Client {
    endpoint: String,
    timeout: Duration,
    conn: Option<Conn>,
    next_id: u64,
    #[cfg(unix)]
    expected_uid: u32,
}

impl Client {
    /// A client of the service at `endpoint` (default: this session's).
    pub fn new(endpoint: Option<&str>) -> Result<Client, ClientError> {
        let endpoint = match endpoint {
            Some(e) => e.to_string(),
            None => crate::ipc::default_endpoint().map_err(|e| {
                ClientError::Connect(tr!(
                    "Can't connect to the Glisa service ({error})",
                    error = error_text(&e)
                ))
            })?,
        };
        Ok(Client {
            endpoint,
            timeout: Duration::from_secs(5),
            conn: None,
            next_id: 1,
            #[cfg(unix)]
            expected_uid: crate::ipc::unix::my_uid(),
        })
    }

    /// Expects the service to run as `uid` (tests simulate another user with it).
    #[cfg(unix)]
    #[doc(hidden)]
    pub fn expect_uid(mut self, uid: u32) -> Self {
        self.expected_uid = uid;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn connect(&mut self) -> Result<(), ClientError> {
        let cant = |e: &std::io::Error| {
            ClientError::Connect(tr!(
                "Can't connect to the Glisa service ({error})",
                error = error_text(e)
            ))
        };
        let not_ours = || {
            ClientError::Connect(tr!(
                "The service at {path} doesn't belong to this user",
                path = self.endpoint
            ))
        };
        #[cfg(unix)]
        let stream = {
            let s = Stream::connect(&self.endpoint).map_err(|e| cant(&e))?;
            let _ = s.set_read_timeout(Some(self.timeout));
            let _ = s.set_write_timeout(Some(self.timeout));
            if crate::ipc::unix::peer_uid(&s) != Some(self.expected_uid) {
                return Err(not_ours());
            }
            s
        };
        #[cfg(windows)]
        let stream = {
            let ms = self.timeout.as_millis().min(u32::MAX as u128) as u32;
            let p = Stream::connect(&self.endpoint, ms).map_err(|e| cant(&e))?;
            if !crate::ipc::windows::owned_by_current_user(p.handle()) {
                return Err(not_ours());
            }
            p
        };
        let stream = std::sync::Arc::new(stream);
        self.conn = Some(Conn {
            reader: BufReader::new(ArcRead(stream.clone())),
            stream,
        });
        Ok(())
    }

    pub fn close(&mut self) {
        self.conn = None;
    }

    /// Calls `method` with `params` (a JSON object, or null).
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let mut line = None;
        for attempt in 0..2 {
            if self.conn.is_none() {
                self.connect()?;
            }
            let id = self.next_id;
            self.next_id += 1;
            let msg = json!({
                "id": id, "method": method, "params": params,
                "lang": glisa_i18n::current_language(),
            });
            let mut bytes = serde_json::to_vec(&msg).unwrap_or_default();
            bytes.push(b'\n');
            let conn = self.conn.as_mut().expect("connected above");
            let result = (&*conn.stream)
                .write_all(&bytes)
                .and_then(|()| (&*conn.stream).flush())
                .and_then(|()| read_line_limited(&mut conn.reader, MAX_LINE));
            match result {
                Ok(Some(l)) => {
                    line = Some(l);
                    break;
                }
                _ => {
                    self.close();
                    if attempt == 1 {
                        return Err(ClientError::Connect(tr!(
                            "Lost the connection to the Glisa service"
                        )));
                    }
                }
            }
        }
        let line = line.unwrap_or_default();
        let resp: Value = serde_json::from_slice(&line)
            .map_err(|_| ClientError::Connect(tr!("Lost the connection to the Glisa service")))?;
        if let Some(e) = resp.get("error") {
            return Err(ClientError::Remote(
                e.as_str().map_or_else(|| e.to_string(), str::to_string),
            ));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }
}
