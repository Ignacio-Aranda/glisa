//! The whole clipboard (every MIME type), not only its text: copied files (`text/uri-list`)
//! and images (`image/png`, …) for Flow transfers.
//!
//! ```text
//!     RichClipboard ...... what the service needs: formats(), read(), read_to(), put()
//!     FakeClipboard ...... tests and demo mode
//!     linux::datacontrol . Wayland data-control (KDE Wayland)
//!     linux::x11 ......... X11 selections
//!     windows / macos .... the system clipboard, translated to and from MIME types
//! ```
//!
//! Every implementation speaks MIME types: a platform clipboard translates its own formats
//! at the edge (Windows `CF_HDROP` ↔ `text/uri-list`, `CF_DIBV5`/"PNG" ↔ `image/png`; macOS
//! file URLs, `public.png`/`public.tiff` ↔ `image/png`), so images travel as PNG and the
//! service never decodes one. `put()` offers data by MIME type: bytes, or a file streamed
//! when someone pastes. Clipboard contents are never logged.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::transfer::{ClipData, Offers};
use crate::util::lock;

/// The clipboard holds more than the caller accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooLarge;

pub trait RichClipboard: Send + Sync {
    /// "data-control", "x11", "windows", "macos", "fake".
    fn kind(&self) -> &'static str;

    fn available(&self) -> bool;

    /// MIME types of the current clipboard (None: unknown).
    fn formats(&self, timeout: Duration) -> Option<Vec<String>>;

    /// Like [`read`](Self::read), written to `out`; the byte count.
    fn read_to(
        &self,
        mime: &str,
        out: &mut dyn Write,
        limit: usize,
        timeout: Duration,
    ) -> Result<Option<u64>, TooLarge>;

    /// The data of one MIME type; None if unavailable; TooLarge above `limit` bytes.
    fn read(
        &self,
        mime: &str,
        limit: usize,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>, TooLarge> {
        let mut buf = Vec::new();
        Ok(self.read_to(mime, &mut buf, limit, timeout)?.map(|_| buf))
    }

    /// Make `offers` (MIME type → data) the clipboard. False if it couldn't.
    fn put(&self, offers: &Offers) -> bool;

    fn close(&self) {}
}

/// In memory (tests, demo mode).
#[derive(Default)]
pub struct FakeClipboard {
    pub data: Mutex<BTreeMap<String, Vec<u8>>>,
    pub puts: Mutex<Vec<Offers>>,
    pub cannot_put: AtomicBool,
}

impl FakeClipboard {
    pub fn set(&self, data: BTreeMap<String, Vec<u8>>) {
        *lock(&self.data) = data;
    }

    pub fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        lock(&self.data).clone()
    }
}

impl RichClipboard for FakeClipboard {
    fn kind(&self) -> &'static str {
        "fake"
    }

    fn available(&self) -> bool {
        true
    }

    fn formats(&self, _timeout: Duration) -> Option<Vec<String>> {
        Some(lock(&self.data).keys().cloned().collect())
    }

    fn read_to(
        &self,
        mime: &str,
        out: &mut dyn Write,
        limit: usize,
        _timeout: Duration,
    ) -> Result<Option<u64>, TooLarge> {
        let data = lock(&self.data).get(mime).cloned();
        match data {
            None => Ok(None),
            Some(d) if d.len() > limit => Err(TooLarge),
            Some(d) => Ok(out.write_all(&d).ok().map(|()| d.len() as u64)),
        }
    }

    fn put(&self, offers: &Offers) -> bool {
        if self.cannot_put.load(Ordering::SeqCst) {
            return false;
        }
        let mut out = BTreeMap::new();
        for (mime, data) in offers {
            match data.bytes() {
                Ok(b) => {
                    out.insert(mime.clone(), b);
                }
                Err(_) => return false,
            }
        }
        lock(&self.puts).push(offers.clone());
        *lock(&self.data) = out;
        true
    }
}

/// Streams `data` into `write` in chunks (a file is read as it goes, never whole).
pub fn copy_out(
    data: &ClipData,
    write: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match data {
        ClipData::Bytes(b) => write(b),
        ClipData::File(p) => {
            use std::io::Read;
            let mut f = std::fs::File::open(p)?;
            let mut buf = vec![0u8; 1 << 16];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    return Ok(());
                }
                write(&buf[..n])?;
            }
        }
    }
}
