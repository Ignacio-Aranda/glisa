//! Copied files and images: one transfer connection per transfer (docs/FLOW.md §6).
//!
//! After a switch the sender reads the clipboard: files (`text/uri-list` of local paths)
//! or an image (PNG preferred) go over a transfer connection when both computers allow it
//! and the peer announced `transfer`; otherwise the text goes as before. The receiver
//! writes into a new private staging folder and puts the result on its clipboard (or
//! leaves it for the app). What was exchanged is remembered per peer, so it isn't sent
//! twice nor sent back.
//!
//! [`FlowService::send_paths`] starts a transfer of given paths directly: files dragged
//! across the edge go this way. On the receiving side, those wait for the pickup's outcome
//! before going on the clipboard (`pickup.rs`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glisa_i18n::{n_, tr};
use serde_json::{Value, json};

use super::FlowService;
use crate::clipboard::TooLarge;
use crate::proto::{self, Conn, Frame, Msg};
use crate::store::Options;
use crate::text::{Text, human_size, plain};
use crate::transfer::{
    self, Incoming, Offers, Outgoing, ScanLimits, SourceKind, URI_LIST_MAX, XferError,
    check_manifest, clipboard_offers, image_offers, image_outgoing, parse_uri_list,
    pick_image_type, pick_text_type, quick_key, scan_files,
};
use crate::util::{lock, random_hex};
use crate::{CAP_TRANSFER, CLIPBOARD_MAX, IMAGE_TYPES, MAX_FILES, imageinfo, tls};

/// Why a transfer was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendReason {
    /// What was copied, after a switch.
    Clipboard,
    /// Dragged across the edge.
    Drag,
}

impl SendReason {
    fn as_str(self) -> &'static str {
        match self {
            SendReason::Clipboard => "clipboard",
            SendReason::Drag => "drag",
        }
    }
}

/// Why the other computer didn't take the files or the image (transfer reason → msgid).
fn transfer_refused(reason: &str) -> &'static str {
    match reason {
        "disabled" => n_!("{name} doesn't accept copied files and images (it's turned off there)"),
        "busy" => n_!("{name} is still receiving something else: try again in a moment"),
        "too_large" => n_!("{name} has a lower size limit (Flow → Options there)"),
        "too_many" => n_!("{name} doesn't accept that many files"),
        "no_space" => n_!("{name} doesn't have enough free space"),
        "cancelled" => n_!("cancelled on {name}"),
        "protocol" => n_!("{name} gave an unexpected answer"),
        "unknown_peer" => n_!("{name} doesn't recognize this computer: pair them again"),
        _ => n_!("saving failed on {name}"),
    }
}

/// "<what> wasn't sent / received: <reason>", per kind of transfer.
fn not_sent(kind: &str) -> &'static str {
    if kind == "image" {
        n_!("The copied image wasn't sent to {name}: {reason}")
    } else {
        n_!("The copied files weren't sent to {name}: {reason}")
    }
}

fn not_received(kind: &str) -> &'static str {
    if kind == "image" {
        n_!("The image from {name} wasn't received: {reason}")
    } else {
        n_!("The files from {name} weren't received: {reason}")
    }
}

const LOST: &str = n_!("the connection was lost");

#[derive(Debug, Clone, Default)]
struct Progress {
    state: &'static str,
    items: u64,
    top_folders: u64,
    files: u64,
    folders: u64,
    skipped: u64,
    failed: u64,
    total: u64,
    done: u64,
    first: String,
    folder: String,
    placed: &'static str,
    cut: bool,
    finished: Option<Instant>,
    last_event: Option<Instant>,
}

/// A received transfer not on the clipboard yet (dragged files waiting for the pickup).
pub(crate) struct Held {
    t: Arc<TransferState>,
    offers: Offers,
    rich: Value,
}

impl Held {
    pub(crate) fn peer(&self) -> &str {
        &self.t.peer
    }
}

/// One transfer, sent or received, as the window and `flow status` see it.
pub struct TransferState {
    pub id: String,
    pub direction: &'static str,
    pub peer: String,
    pub name: String,
    pub kind: String,
    pub reason: SendReason,
    pub cancel: AtomicBool,
    m: Mutex<Progress>,
}

impl TransferState {
    fn new(
        direction: &'static str,
        peer: &str,
        name: &str,
        kind: &str,
        reason: SendReason,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: random_hex(6),
            direction,
            peer: peer.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            reason,
            cancel: AtomicBool::new(false),
            m: Mutex::new(Progress {
                state: if direction == "send" {
                    "preparing"
                } else {
                    "receiving"
                },
                ..Default::default()
            }),
        })
    }

    pub fn active(&self) -> bool {
        matches!(
            lock(&self.m).state,
            "preparing" | "waiting" | "sending" | "receiving"
        )
    }

    fn set<F: FnOnce(&mut Progress)>(&self, f: F) {
        f(&mut lock(&self.m));
    }

    fn end(&self, state: &'static str) {
        let mut m = lock(&self.m);
        m.state = state;
        m.finished = Some(Instant::now());
    }

    pub fn state(&self) -> &'static str {
        lock(&self.m).state
    }

    pub fn snapshot(&self) -> Value {
        let m = lock(&self.m).clone();
        let active = matches!(m.state, "preparing" | "waiting" | "sending" | "receiving");
        let pct = match m.done.saturating_mul(100).checked_div(m.total) {
            None if !active => 100,
            None => 0,
            Some(p) => p.min(100),
        };
        let ago = m
            .finished
            .map(|t| (t.elapsed().as_secs_f64() * 10.0).round() / 10.0)
            .unwrap_or(0.0);
        json!({"id": self.id, "direction": self.direction, "peer": self.peer, "name": self.name,
               "kind": self.kind, "state": m.state, "active": active, "items": m.items,
               "top_folders": m.top_folders, "files": m.files, "folders": m.folders,
               "bytes": m.total, "done": m.done, "percent": pct, "failed": m.failed,
               "skipped": m.skipped, "first": m.first, "folder": m.folder, "placed": m.placed,
               "cut": m.cut, "ago": ago, "reason": self.reason.as_str()})
    }
}

impl FlowService {
    fn add_transfer(&self, t: Arc<TransferState>) {
        let mut st = lock(&self.st);
        st.transfers.push_back(t);
        while st.transfers.len() > 8 {
            st.transfers.pop_front();
        }
    }

    /// A transfer's progress moved: tell the window (at most 4 times a second).
    fn progress(&self, t: &TransferState, n: u64) {
        let due = {
            let mut m = lock(&t.m);
            m.done += n;
            let due = m
                .last_event
                .is_none_or(|e| e.elapsed() > Duration::from_millis(250));
            if due {
                m.last_event = Some(Instant::now());
            }
            due
        };
        if due {
            self.emit("transfer", t.snapshot());
        }
    }

    fn transfer_changed(&self, t: &TransferState) {
        self.emit("transfer", t.snapshot());
    }

    pub fn transfers_status(&self) -> Vec<Value> {
        let ts: Vec<Arc<TransferState>> = lock(&self.st).transfers.iter().cloned().collect();
        ts.iter().map(|t| t.snapshot()).collect()
    }

    pub fn cancel_transfer(&self, tid: &str) -> bool {
        let t = lock(&self.st)
            .transfers
            .iter()
            .find(|t| t.id == tid)
            .cloned();
        match t {
            Some(t) if t.active() => {
                t.cancel.store(true, Ordering::SeqCst);
                true
            }
            _ => false,
        }
    }

    #[doc(hidden)]
    pub fn cleanup_staging_for_tests(&self) {
        self.cleanup_staging();
    }

    pub(crate) fn cleanup_staging(&self) {
        let keep: Vec<String> = {
            let mut st = lock(&self.st);
            st.last_cleanup = Some(Instant::now());
            st.transfers
                .iter()
                .filter(|t| t.active())
                .filter_map(|t| {
                    let f = lock(&t.m).folder.clone();
                    std::path::Path::new(&f)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .chain([".spool".to_string()])
                .collect()
        };
        let removed = self.staging.cleanup(transfer::STAGING_TTL, &keep);
        if removed > 0 {
            log::info!("Flow: {removed} received transfer(s) older than a day deleted");
        }
    }

    // ================================================================ receiving
    /// Receive copied files or an image (we are the server of a transfer connection).
    /// Everything lands in a new private staging folder; on any error it is deleted.
    pub(crate) fn serve_transfer(&self, conn: &mut Conn, pid: &str, pin: &str, m: Msg) {
        conn.set_types(proto::TRANSFER_CLIENT, proto::TRANSFER_SERVER, true);
        let name = self
            .store
            .peer(pid)
            .map(|p| p.name)
            .unwrap_or_else(|| pid.to_string());
        let opts = self.store.options();
        let (max_bytes, max_image) = (opts.max_transfer_bytes(), opts.max_image_bytes());
        let kind = m["kind"].as_str().unwrap_or("files").to_string();
        let entries: Vec<Value> = m["entries"].as_array().cloned().unwrap_or_default();
        let text = m["text"].as_str().unwrap_or("").to_string();
        let seq = m["seq"].clone();
        let refuse = |conn: &mut Conn, reason: &str, notice: Option<Text>| {
            let _ = conn.send("transfer_refused", json!({"re": seq, "reason": reason}));
            log::info!("Flow: refused a transfer from {name} ({reason})");
            if let Some(n) = notice {
                self.notice(
                    n,
                    "warn",
                    Some(format!("refused-files:{pid}")),
                    Duration::from_secs(10),
                );
            }
        };
        // Files dragged across the edge: the transfer its `drag` notice announced.
        let reason = if self.drag_expects(pid, &kind, &entries) {
            SendReason::Drag
        } else {
            SendReason::Clipboard
        };
        let st = TransferState::new("receive", pid, &name, &kind, reason);
        let top = |e: &Value| !e["path"].as_str().unwrap_or("/").contains('/');
        st.set(|p| {
            p.items = if kind == "files" {
                entries.iter().filter(|e| top(e)).count() as u64
            } else {
                1
            };
            p.files = entries.iter().filter(|e| e["type"] == "file").count() as u64;
            p.folders = entries.iter().filter(|e| e["type"] == "dir").count() as u64;
            p.total = entries.iter().filter_map(|e| e["size"].as_u64()).sum();
            p.first = entries
                .iter()
                .find(|e| top(e))
                .and_then(|e| e["path"].as_str())
                .unwrap_or("")
                .to_string();
            p.top_folders = entries
                .iter()
                .filter(|e| e["type"] == "dir" && top(e))
                .count() as u64;
        });
        let failed = |reason: Text, k: &'static str| {
            self.notice(
                Text::new(not_received(&kind))
                    .arg("name", plain(name.clone()))
                    .arg("reason", reason),
                k,
                None,
                Duration::ZERO,
            );
        };
        if !self.running() || !opts.files {
            refuse(conn, "disabled", None);
            return;
        }
        {
            let mut stt = lock(&self.st);
            if stt
                .transfers
                .iter()
                .any(|t| t.active() && t.direction == "receive")
            {
                drop(stt);
                refuse(conn, "busy", None);
                return;
            }
            stt.transfers.push_back(st.clone());
            while stt.transfers.len() > 8 {
                stt.transfers.pop_front();
            }
        }
        self.transfer_changed(&st);
        let err = match self.staging.prepare() {
            Ok(_) => check_manifest(
                &kind,
                &entries,
                max_bytes,
                max_image,
                self.staging.free_bytes(),
            ),
            Err(e) => {
                log::warn!("Flow: staging folder: {e}");
                Some(XferError::refused("error", None))
            }
        };
        if let Some(XferError::Refused { reason, .. }) = err {
            st.end("refused");
            let limit = human_size(if kind == "image" {
                max_image
            } else {
                max_bytes
            });
            let why = match reason {
                "too_large" => Some(
                    Text::new(n_!("your limit is {limit} (Flow → Options)")).arg("limit", limit),
                ),
                "no_space" => Some(Text::new(n_!("there isn't enough free space"))),
                _ => None,
            };
            let notice = why.map(|w| {
                Text::new(not_received(&kind))
                    .arg("name", plain(name.clone()))
                    .arg("reason", w)
            });
            refuse(conn, reason, notice);
            self.transfer_changed(&st);
            return;
        }
        let mut inc: Option<Incoming> = None;
        let result = (|| -> Result<(), XferError> {
            let (_tid, folder) = self.staging.new_folder()?;
            st.set(|p| p.folder = folder.to_string_lossy().into_owned());
            let i = inc.insert(Incoming::new(folder, &kind, entries.clone(), &text));
            i.create()?;
            conn.send("transfer_ok", json!({"re": seq}))?;
            let mut idle = Duration::ZERO;
            loop {
                if st.cancel.load(Ordering::SeqCst) || !self.running() {
                    return Err(XferError::cancelled());
                }
                let f = match conn.recv(Some(Duration::from_millis(500))) {
                    Err(proto::Error::Idle) => {
                        idle += Duration::from_millis(500);
                        if idle > transfer::IDLE_TIMEOUT {
                            return Err(XferError::Io(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "the sender stopped",
                            )));
                        }
                        continue;
                    }
                    other => other?,
                };
                idle = Duration::ZERO;
                if self.store.peer_by_pin(pin).as_deref() != Some(pid) {
                    return Err(XferError::Cancelled {
                        reason: "unknown_peer".into(),
                        by_peer: false,
                    });
                }
                match f {
                    Frame::Data(d) => {
                        let before = i.received;
                        i.data(d.index, d.offset, &d.data)?;
                        self.progress(&st, i.received - before);
                    }
                    Frame::Msg(fm) => match fm["type"].as_str().unwrap_or("") {
                        "transfer_skip" => {
                            let before = i.received;
                            i.skip(fm["index"].as_u64().unwrap_or(0) as u32)?;
                            self.progress(&st, i.received - before);
                        }
                        "transfer_cancel" => {
                            return Err(XferError::Cancelled {
                                reason: fm["reason"].as_str().unwrap_or("cancelled").to_string(),
                                by_peer: true,
                            });
                        }
                        "transfer_end" => {
                            i.end()?;
                            if kind == "image" {
                                i.check_image(max_image)?;
                            }
                            let n = i.failed.len() as u64;
                            st.set(|p| p.failed = n);
                            conn.send("transfer_done", json!({"re": fm["seq"], "failed": n}))?;
                            return Ok(());
                        }
                        t => return Err(XferError::Protocol(format!("{t} out of place"))),
                    },
                }
            }
        })();
        match result {
            Ok(()) => {
                if let Some(i) = inc.as_mut() {
                    self.received(pid, &st, i);
                }
            }
            Err(e) => {
                if let Some(i) = inc.as_mut() {
                    i.abort();
                }
                match e {
                    XferError::Cancelled { by_peer, .. } => {
                        st.end("cancelled");
                        if by_peer {
                            failed(
                                Text::new(n_!("{name} cancelled it"))
                                    .arg("name", plain(name.clone())),
                                "info",
                            );
                        } else {
                            let _ = conn.send("transfer_cancel", json!({"reason": "cancelled"}));
                        }
                    }
                    XferError::Refused { reason, message } => {
                        st.end("failed");
                        let _ = conn.send("transfer_cancel", json!({"reason": reason}));
                        failed(
                            message.unwrap_or_else(|| Text::new(n_!("it couldn't be saved"))),
                            "warn",
                        );
                    }
                    XferError::Protocol(p) => {
                        st.end("failed");
                        log::info!("Flow: {pid} broke the transfer protocol: {p}");
                        let _ = conn.send("error", json!({"reason": "protocol"}));
                        failed(
                            Text::new(n_!("what arrived wasn't valid, so it was deleted")),
                            "warn",
                        );
                    }
                    XferError::Io(e) => {
                        st.end("failed");
                        let no_space = e.raw_os_error() == Some(28)
                            || e.kind() == std::io::ErrorKind::StorageFull;
                        let local = !matches!(
                            e.kind(),
                            std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::UnexpectedEof
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::NotConnected
                                | std::io::ErrorKind::WouldBlock
                        );
                        if local {
                            let _ = conn.send(
                                "transfer_cancel",
                                json!({"reason": if no_space { "no_space" } else { "failed" }}),
                            );
                        }
                        log::info!("Flow: receiving files from {pid} failed: {e}");
                        failed(
                            if no_space {
                                Text::new(n_!("there isn't enough free space"))
                            } else {
                                Text::new(LOST)
                            },
                            "warn",
                        );
                    }
                }
            }
        }
        self.transfer_changed(&st);
    }

    /// A transfer arrived: put it on this computer's clipboard (or leave it for the app),
    /// and remember it so it isn't sent back.
    fn received(&self, pid: &str, st: &TransferState, inc: &mut Incoming) {
        let paths = inc.top_level();
        let name = st.name.clone();
        if paths.is_empty() {
            st.end("failed");
            self.notice(
                Text::new(not_received(&st.kind))
                    .arg("name", plain(name))
                    .arg("reason", Text::new(n_!("nothing arrived intact"))),
                "warn",
                None,
                Duration::ZERO,
            );
            if let Some(tid) = inc
                .folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
            {
                self.staging.remove(&tid);
            }
            return;
        }
        let (offers, rich) = if inc.kind == "image" {
            let mime = inc.entries[0]["mime"]
                .as_str()
                .unwrap_or("image/png")
                .to_string();
            let path = inc.image_path.clone().unwrap_or_default();
            {
                let mut s = lock(&self.st);
                s.sent_keys.insert(
                    pid.to_string(),
                    inc.entries[0]["sha256"].as_str().unwrap_or("").to_string(),
                );
                if !inc.text.is_empty() {
                    s.clip_hash
                        .insert(pid.to_string(), transfer::text_key(&inc.text));
                }
            }
            (
                image_offers(&path, &mime, &inc.text),
                json!({"kind": "image", "image": {"path": path.to_string_lossy(), "mime": mime},
                       "text": inc.text, "uris": []}),
            )
        } else {
            lock(&self.st)
                .sent_keys
                .insert(pid.to_string(), quick_key(&paths));
            let text: Vec<String> = paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            (
                clipboard_offers(&paths),
                json!({"kind": "files", "uris": paths.iter().map(|p| transfer::uri_of(p)).collect::<Vec<_>>(),
                       "text": text.join("\n"), "image": null}),
            )
        };
        let failed = lock(&st.m).failed;
        if failed > 0 {
            self.notice(
                Text::new(n_!(
                    "{count} of the files from {name} didn't arrive intact and were left out"
                ))
                .arg("count", failed)
                .arg("name", plain(name.clone())),
                "warn",
                None,
                Duration::ZERO,
            );
        }
        log::info!(
            "Flow: received {} from {} ({} bytes)",
            inc.kind,
            name,
            lock(&st.m).total
        );
        let t = lock(&self.st)
            .transfers
            .iter()
            .find(|t| t.id == st.id)
            .cloned();
        let Some(t) = t else { return };
        let mut held = Some(Held { t, offers, rich });
        if st.reason == SendReason::Drag {
            // Dragged here: a drag picked up on this desktop may take them first.
            held = held.and_then(|h| self.pickup_hold(h, &paths));
        }
        if let Some(h) = held {
            self.place_received(h);
        }
    }

    /// Puts a received transfer on this computer's clipboard (or leaves it for the app's
    /// fallback): done, paste with Ctrl+V.
    pub(crate) fn place_received(&self, h: Held) {
        let clip = self.backend().and_then(|b| b.rich());
        let placed = clip.is_some_and(|c| c.available() && c.put(&h.offers));
        if placed {
            h.t.set(|p| p.placed = "clipboard");
        } else {
            {
                // the app's clipboard fallback takes it (flow_clipboard_take)
                let mut s = lock(&self.st);
                s.gui_clip_seq += 1;
                s.gui_clip_text = h.rich["text"].as_str().unwrap_or("").to_string();
                s.gui_clip_rich = Some(h.rich);
            }
            h.t.set(|p| p.placed = "gui");
            self.emit_flow("clipboard");
        }
        h.t.end("done");
        self.transfer_changed(&h.t);
    }

    /// The dragged files were dropped somewhere here by a drag picked up on this desktop:
    /// done, nothing on the clipboard.
    pub(crate) fn finish_dropped(&self, h: Held) {
        h.t.set(|p| p.placed = "drop");
        h.t.end("done");
        self.transfer_changed(&h.t);
    }

    // ================================================================ sending
    /// What is on the clipboard besides text: files (paths, cut) or an image (MIME type,
    /// text MIME type); None when there is neither (or it can't be known).
    fn rich_snapshot(&self) -> Option<RichClip> {
        if let Some(clip) = self
            .backend()
            .and_then(|b| b.rich())
            .filter(|c| c.available())
        {
            let formats = clip.formats(Duration::from_millis(500))?;
            if formats.is_empty() {
                return None;
            }
            if formats.iter().any(|f| f == "text/uri-list") {
                let data = clip
                    .read("text/uri-list", URI_LIST_MAX, Duration::from_secs(1))
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let paths = parse_uri_list(&data, MAX_FILES);
                if !paths.is_empty() {
                    let cut = formats
                        .iter()
                        .any(|f| f == "application/x-kde-cutselection")
                        && clip
                            .read(
                                "application/x-kde-cutselection",
                                16,
                                Duration::from_millis(500),
                            )
                            .ok()
                            .flatten()
                            .is_some_and(|d| {
                                d.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'1')
                            });
                    return Some(RichClip::Files { paths, cut });
                }
            }
            if let Some(mime) = pick_image_type(&formats) {
                return Some(RichClip::Image {
                    mime,
                    text_mime: pick_text_type(&formats),
                });
            }
            return None;
        }
        let uris = lock(&self.st).gui_clip_uris.clone();
        let paths = parse_uri_list(uris.join("\n").as_bytes(), MAX_FILES);
        (!paths.is_empty()).then_some(RichClip::Files { paths, cut: false })
    }

    /// The device went to `pid`: send what was copied. Files or an image go over a
    /// transfer connection (when both computers allow it and the peer supports it);
    /// otherwise the text, as always.
    pub(crate) fn after_switch(&self, pid: &str) {
        let opts = self.store.options();
        let mut handled = false;
        if opts.files {
            handled = self.send_rich(pid, &opts);
        }
        if !handled && opts.clipboard {
            self.send_clipboard(pid);
        }
    }

    /// True when the clipboard holds files or an image and that is dealt with here (sent,
    /// already there, or refused with a notice).
    pub fn send_rich(&self, pid: &str, opts: &Options) -> bool {
        let Some(link) = self.link(pid) else {
            return false;
        };
        if !link.caps().contains(CAP_TRANSFER) {
            return false; // an older Glisa there: text only
        }
        let Some(clip) = self.rich_snapshot() else {
            return false;
        };
        let name = self
            .store
            .peer(pid)
            .map(|p| p.name)
            .unwrap_or_else(|| pid.to_string());
        let busy = lock(&self.st)
            .transfers
            .iter()
            .any(|t| t.active() && t.direction == "send");
        match clip {
            RichClip::Files { paths, cut } => {
                let key = quick_key(&paths);
                if lock(&self.st).sent_keys.get(pid) == Some(&key) {
                    return true; // already there (or it came from there)
                }
                if busy {
                    self.notice(
                        Text::new(n_!(
                            "Still sending the previous files to {name}: the new ones weren't sent"
                        ))
                        .arg("name", plain(name)),
                        "info",
                        Some(format!("busy-send:{pid}")),
                        Duration::from_secs(5),
                    );
                    return true;
                }
                let st = TransferState::new("send", pid, &name, "files", SendReason::Clipboard);
                st.set(|p| p.cut = cut);
                self.start_send(st, SendJob::Files(paths), key, opts.clone());
                true
            }
            RichClip::Image { mime, text_mime } => {
                if busy {
                    return false;
                }
                self.send_image(pid, &name, mime, text_mime, opts)
            }
        }
    }

    fn send_image(
        &self,
        pid: &str,
        name: &str,
        mime: &str,
        text_mime: Option<&str>,
        opts: &Options,
    ) -> bool {
        // read it now (the clipboard may change), then send it in the background
        let Some(board) = self.backend().and_then(|b| b.rich()) else {
            return false;
        };
        let Ok(spool) = self.staging.spool_file() else {
            return false;
        };
        let limit = opts.max_image_bytes();
        let read = (|| -> Result<Option<u64>, TooLarge> {
            let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&spool) else {
                return Ok(None);
            };
            board.read_to(mime, &mut f, limit as usize, Duration::from_secs(5))
        })();
        let n = match read {
            Err(TooLarge) => {
                let _ = std::fs::remove_file(&spool);
                self.notice(
                    Text::new(n_!("The copied image is larger than {limit}, the limit for sending it (Flow → Options)"))
                        .arg("limit", human_size(limit)),
                    "info",
                    Some("image-size".into()),
                    Duration::from_secs(30),
                );
                return true;
            }
            Ok(n) => n.unwrap_or(0),
        };
        if n == 0 {
            let _ = std::fs::remove_file(&spool);
            return false;
        }
        let checked = imageinfo::inspect(&spool)
            .ok()
            .filter(|(m, w, h)| IMAGE_TYPES.contains(m) && imageinfo::acceptable(*w, *h));
        let Some((mime, _, _)) = checked else {
            let _ = std::fs::remove_file(&spool);
            log::info!("Flow: the copied image isn't one Flow sends (type or size)");
            return false;
        };
        let mut text = String::new();
        if opts.clipboard {
            if let Some(tm) = text_mime {
                text = board
                    .read(tm, CLIPBOARD_MAX, Duration::from_secs(1))
                    .ok()
                    .flatten()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default();
            }
        }
        let Ok(out) = image_outgoing(&spool, mime, &text) else {
            let _ = std::fs::remove_file(&spool);
            return false;
        };
        let key = out.sources[0].sha256.clone();
        if lock(&self.st).sent_keys.get(pid) == Some(&key) {
            let _ = std::fs::remove_file(&spool);
            return true;
        }
        let st = TransferState::new("send", pid, name, "image", SendReason::Clipboard);
        st.set(|p| {
            p.items = 1;
            p.total = out.total();
        });
        self.start_send(st, SendJob::Ready(out), key, opts.clone());
        true
    }

    /// Sends files or folders to a paired computer now (no clipboard involved). Returns
    /// the transfer id. Used by drag and drop across the edge.
    pub fn send_paths(
        &self,
        pid: &str,
        paths: Vec<PathBuf>,
        reason: SendReason,
    ) -> Result<String, String> {
        if self.store.peer(pid).is_none() {
            return Err(tr!("That computer isn't paired"));
        }
        let link = self.link(pid).filter(|l| l.caps().contains(CAP_TRANSFER));
        if link.is_none() {
            return Err(tr!(
                "{name} doesn't accept copied files and images (it's turned off there)",
                name = pid
            ));
        }
        let name = self
            .store
            .peer(pid)
            .map(|p| p.name)
            .unwrap_or_else(|| pid.to_string());
        let st = TransferState::new("send", pid, &name, "files", reason);
        let id = st.id.clone();
        let key = quick_key(&paths);
        self.start_send(st, SendJob::Files(paths), key, self.store.options());
        Ok(id)
    }

    fn start_send(&self, st: Arc<TransferState>, job: SendJob, key: String, opts: Options) {
        self.add_transfer(st.clone());
        self.transfer_changed(&st);
        let svc = self.arc();
        let spawned = std::thread::Builder::new()
            .name("flow-send".into())
            .spawn(move || svc.run_send(st, job, key, opts));
        if spawned.is_err() {
            log::warn!("Flow: couldn't start sending");
        }
    }

    /// A new connection for one transfer: same address, TLS and pin as the link.
    fn transfer_connect(&self, pid: &str) -> Option<Conn> {
        let peer = self.store.peer(pid)?;
        let link = self.link(pid)?;
        let id = self.identity()?;
        let cfg = tls::client_config(&id, &peer.pin).ok()?;
        let live = link.addr();
        let mut addrs: Vec<(String, u16)> = live.clone().into_iter().collect();
        for (a, _) in link.candidates(Some(&peer)) {
            if Some(&a) != live.as_ref() {
                addrs.push(a);
            }
        }
        for (host, port) in addrs {
            if let Ok(c) = tls::connect(
                &host,
                port,
                cfg.clone(),
                Duration::from_secs(3),
                proto::TRANSFER_SERVER,
                proto::TRANSFER_SENDER,
            ) {
                return Some(c);
            }
        }
        None
    }

    fn run_send(&self, st: Arc<TransferState>, job: SendJob, key: String, opts: Options) {
        let name = st.name.clone();
        let hooks = lock(&self.hooks).clone();
        let mut conn: Option<Conn> = None;
        let mut out: Option<Outgoing> = None;
        let result = (|| -> Result<(), SendErr> {
            let o = match job {
                SendJob::Ready(o) => out.insert(o),
                SendJob::Files(paths) => {
                    let limits = ScanLimits {
                        max_bytes: opts.max_transfer_bytes(),
                        max_files: hooks.max_files.unwrap_or(MAX_FILES),
                    };
                    let mut o = scan_files(&paths, limits, &st.cancel)?;
                    let top_folders = o
                        .sources
                        .iter()
                        .filter(|s| s.kind == SourceKind::Dir && !s.rel.contains('/'))
                        .count() as u64;
                    st.set(|p| {
                        p.items = o.items;
                        p.files = o.files;
                        p.folders = o.folders;
                        p.first = o.first.clone();
                        p.top_folders = top_folders;
                        p.total = o.total();
                    });
                    self.transfer_changed(&st);
                    transfer::hash_sources(&mut o, &st.cancel, &|n| self.progress(&st, n))?;
                    st.set(|p| {
                        p.files = o.files;
                        p.skipped = o.skipped;
                        p.total = o.total();
                    });
                    out.insert(o)
                }
            };
            st.set(|p| p.state = "waiting");
            self.transfer_changed(&st);
            let c = conn.insert(
                self.transfer_connect(&st.peer)
                    .ok_or(SendErr::Unreachable)?,
            );
            c.send("transfer", json!({"id": self.store.id(), "kind": o.kind, "entries": o.manifest(), "text": o.text}))?;
            let m = c.recv_msg(Some(Duration::from_secs(10)))?;
            if m["type"] == "transfer_refused" {
                let reason = m["reason"].as_str().unwrap_or("error").to_string();
                st.end("refused");
                let why = Text::new(transfer_refused(&reason)).arg("name", plain(name.clone()));
                self.notice(
                    Text::new(not_sent(&st.kind))
                        .arg("name", plain(name.clone()))
                        .arg("reason", why),
                    "warn",
                    None,
                    Duration::ZERO,
                );
                if reason == "disabled" && o.kind == "image" && !o.text.is_empty() && opts.clipboard
                {
                    self.send_clipboard(&st.peer);
                }
                return Ok(());
            }
            if m["type"] != "transfer_ok" {
                return Err(XferError::Protocol(format!("unexpected answer {}", m["type"])).into());
            }
            st.set(|p| {
                p.state = "sending";
                p.done = 0;
            });
            self.transfer_changed(&st);
            let failed =
                transfer::send_entries(c, o, &st.cancel, &|n| self.progress(&st, n), &hooks.send)?;
            c.send("transfer_end", json!({}))?;
            let deadline = Instant::now() + transfer::DONE_TIMEOUT;
            let m = loop {
                if st.cancel.load(Ordering::SeqCst) {
                    return Err(XferError::cancelled().into());
                }
                match c.recv_msg(Some(Duration::from_millis(500))) {
                    Err(proto::Error::Idle) => {
                        if Instant::now() > deadline {
                            return Err(XferError::Io(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "no answer",
                            ))
                            .into());
                        }
                    }
                    other => break other?,
                }
            };
            if m["type"] == "transfer_cancel" {
                return Err(XferError::Cancelled {
                    reason: m["reason"].as_str().unwrap_or("cancelled").to_string(),
                    by_peer: true,
                }
                .into());
            }
            if m["type"] != "transfer_done" {
                return Err(XferError::Protocol(format!("unexpected answer {}", m["type"])).into());
            }
            let n = (failed as u64).max(m["failed"].as_u64().unwrap_or(0));
            st.set(|p| p.failed = n);
            st.end("done");
            {
                let mut s = lock(&self.st);
                s.sent_keys.insert(st.peer.clone(), key.clone());
                if o.kind == "image" && !o.text.is_empty() {
                    s.clip_hash
                        .insert(st.peer.clone(), transfer::text_key(&o.text));
                }
            }
            if n > 0 {
                self.notice(
                    Text::new(n_!(
                        "{count} of the copied files couldn't be sent to {name}: they changed or disappeared while sending"
                    ))
                    .arg("count", n)
                    .arg("name", plain(name.clone())),
                    "warn",
                    None,
                    Duration::ZERO,
                );
            }
            log::info!("Flow: sent {} to {} ({} bytes)", o.kind, name, o.total());
            Ok(())
        })();
        match result {
            Ok(()) => {}
            Err(SendErr::Unreachable) => {
                st.end("failed");
                let why =
                    Text::new(n_!("{name} can't be reached")).arg("name", plain(name.clone()));
                self.notice(
                    Text::new(not_sent(&st.kind))
                        .arg("name", plain(name.clone()))
                        .arg("reason", why),
                    "warn",
                    None,
                    Duration::ZERO,
                );
            }
            Err(SendErr::Xfer(XferError::Cancelled { reason, by_peer })) => {
                st.end("cancelled");
                if by_peer {
                    let why = Text::new(transfer_refused(if reason == "error" {
                        "cancelled"
                    } else {
                        &reason
                    }))
                    .arg("name", plain(name.clone()));
                    self.notice(
                        Text::new(not_sent(&st.kind))
                            .arg("name", plain(name.clone()))
                            .arg("reason", why),
                        if reason != "cancelled" {
                            "warn"
                        } else {
                            "info"
                        },
                        None,
                        Duration::ZERO,
                    );
                } else if let Some(c) = conn.as_mut() {
                    let _ = c.send("transfer_cancel", json!({"reason": "cancelled"}));
                }
            }
            Err(SendErr::Xfer(XferError::Refused { message, .. })) => {
                st.end("failed");
                let msg = message.unwrap_or_else(|| {
                    Text::new(not_sent(&st.kind))
                        .arg("name", plain(name.clone()))
                        .arg("reason", Text::new(LOST))
                });
                self.notice(msg, "warn", None, Duration::ZERO);
            }
            Err(SendErr::Xfer(e)) => {
                st.end("failed");
                log::info!("Flow: sending files to {} failed: {e}", st.peer);
                self.notice(
                    Text::new(not_sent(&st.kind))
                        .arg("name", plain(name.clone()))
                        .arg("reason", Text::new(LOST)),
                    "warn",
                    None,
                    Duration::ZERO,
                );
            }
        }
        if let Some(mut c) = conn {
            c.close();
        }
        if let Some(o) = out {
            if o.kind == "image" {
                let _ = std::fs::remove_file(&o.sources[0].path);
            }
        }
        self.transfer_changed(&st);
    }
}

enum RichClip {
    Files {
        paths: Vec<PathBuf>,
        cut: bool,
    },
    Image {
        mime: &'static str,
        text_mime: Option<&'static str>,
    },
}

enum SendJob {
    Files(Vec<PathBuf>),
    Ready(Outgoing),
}

enum SendErr {
    Unreachable,
    Xfer(XferError),
}

impl From<XferError> for SendErr {
    fn from(e: XferError) -> Self {
        SendErr::Xfer(e)
    }
}

impl From<proto::Error> for SendErr {
    fn from(e: proto::Error) -> Self {
        SendErr::Xfer(e.into())
    }
}
