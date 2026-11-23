//! KDE drops that copy without asking (KDE Plasma on Wayland, docs/FLOW.md §6b).
//!
//! Files dropped on Dolphin or the Plasma desktop get KIO's menu (*Copy here / Link here /
//! Cancel*): KIO always offers a link. A link into Glisa's staging folder would break when
//! it is cleaned up, so the drag picked up here (catcher.rs) also offers Ark's two MIME
//! types, as Ark does for files dragged out of an archive:
//!
//! * [`SERVICE_MIME`]: this process's unique name on the session bus;
//! * [`PATH_MIME`]: an object of this drag only, `<DBUS_DROP_PATH>/<32 random hex>`.
//!
//! With both, KIO's `DropJob` (the desktop) and Dolphin skip the menu and call
//! `org.kde.ark.DndExtract.extractSelectedFilesTo(destination)` on that object: the source
//! does the copy. [`DndExtract`] does it here:
//!
//! * only for a caller of this user (the bus says its uid), once: the first call takes the
//!   object off the bus, a second one finds nothing;
//! * into an existing local folder this user can write to (an absolute path or a `file:`
//!   URI), canonicalized and opened once; never one inside the dragged files;
//! * once every file of the drag is here ([`ArkDrop::ready`], at most [`FILES_WAIT`]);
//! * a **copy** of the received items, folders recursively: never a move, nothing
//!   overwritten (a name already there becomes `name (2).ext`, as in transfers), no link
//!   followed (every component opened relative to its parent with `O_NOFOLLOW`; links and
//!   special files are skipped), everything created relative to the opened folder with
//!   `O_EXCL`; modes as staged without execute bits (0600 files, 0700 folders), times kept.
//!   On an error, what the call created is removed again and the caller gets a D-Bus error.
//!
//! Dolphin waits for the answer inside its drop handler, so it comes within [`REPLY_WAIT`]:
//! the copy's result, or success while the copy goes on (files still arriving, a big copy).
//! The drag counts as taken only if the copy succeeds ([`ArkDrop::taken_after_drop`]);
//! otherwise the files go on the clipboard, as after any drop that didn't take them.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::future::Future;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use rustix::fs::{Access, AtFlags, FileType, Mode, OFlags, Stat, Timespec, Timestamps};
use rustix::io::Errno;

use crate::util::lock;

/// What a drop target reads: the bus name to call.
pub const SERVICE_MIME: &str = "application/x-kde-ark-dndextract-service";
/// What a drop target reads: the object to call.
pub const PATH_MIME: &str = "application/x-kde-ark-dndextract-path";
/// The interface below must have this name (the macro needs a literal; a test checks).
pub const INTERFACE: &str = "org.kde.ark.DndExtract";
/// How long the call waits for every file of the drag to be here.
pub const FILES_WAIT: Duration = Duration::from_secs(60);
/// The caller has its answer within this (Dolphin waits for it in its drop handler).
pub const REPLY_WAIT: Duration = Duration::from_secs(2);
/// After the drop, how long a target that read the object may take to call it (KIO calls
/// right after the drop, Dolphin during it).
pub const CALL_WAIT: Duration = Duration::from_secs(3);
/// After the call, how long the drag's outcome waits for the copy.
pub const OUTCOME_WAIT: Duration = Duration::from_secs(90);

/// The KDE drop of one drag picked up here.
pub struct ArkDrop {
    path: String,
    st: Mutex<DropState>,
    cv: Condvar,
}

#[derive(Default)]
struct DropState {
    /// The received items (top-level, in their staging folder), once all here.
    files: Option<Vec<PathBuf>>,
    /// The pickup is over: a call still waiting for the files stops.
    over: bool,
    /// A drop target read the object path.
    offered: bool,
    /// The call came (one per drag).
    claimed: bool,
    /// How the call's copy went: items created, or why not.
    outcome: Option<Result<usize, String>>,
}

impl ArkDrop {
    /// A new drop whose object is `<prefix>/<32 random hex>`.
    pub fn new(prefix: &str) -> Arc<Self> {
        Arc::new(Self {
            path: format!("{prefix}/{}", crate::util::random_hex(16)),
            st: Mutex::default(),
            cv: Condvar::new(),
        })
    }

    /// Its object path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Every file of the drag is here: the received items.
    pub fn ready(&self, files: Vec<PathBuf>) {
        lock(&self.st).files = Some(files);
        self.cv.notify_all();
    }

    /// A drop target asked for `mime` (told before it is written).
    pub fn sent(&self, mime: &str) {
        if mime == PATH_MIME {
            lock(&self.st).offered = true;
        }
    }

    /// Did a drop target read the object path (so a call may come)?
    pub fn offered(&self) -> bool {
        lock(&self.st).offered
    }

    /// The one call of this drag: true for the first only.
    pub fn claim(&self) -> bool {
        let mut s = lock(&self.st);
        let first = !s.claimed;
        s.claimed = true;
        drop(s);
        self.cv.notify_all();
        first
    }

    pub fn claimed(&self) -> bool {
        lock(&self.st).claimed
    }

    /// The pickup is over: a call still waiting for the files gives up.
    pub fn retire(&self) {
        lock(&self.st).over = true;
        self.cv.notify_all();
    }

    /// The call: checks `destination`, waits (at most `files_wait`) for the files, copies
    /// them there. The outcome is kept for [`ArkDrop::taken_after_drop`].
    pub fn extract(&self, destination: &str, files_wait: Duration) -> Result<usize, String> {
        let r = Destination::open(destination).and_then(|dest| {
            let files = self.wait_files(files_wait)?;
            copy_items(&files, &dest)
        });
        match &r {
            Ok(n) => log::info!("Flow: pickup: dropped on KDE: {n} item(s) copied there"),
            Err(e) => log::info!("Flow: pickup: dropped on KDE, but not copied: {e}"),
        }
        lock(&self.st).outcome = Some(r.clone());
        self.cv.notify_all();
        r
    }

    fn wait_files(&self, wait: Duration) -> Result<Vec<PathBuf>, String> {
        let end = Instant::now() + wait;
        let mut s = lock(&self.st);
        loop {
            if let Some(f) = &s.files {
                return Ok(f.clone());
            }
            if s.over {
                return Err("the drag is over and its files aren't all here".into());
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("the files didn't all arrive in time".into());
            }
            s = self
                .cv
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The copy's outcome, waiting at most `wait` for it.
    pub fn wait_outcome(&self, wait: Duration) -> Option<Result<usize, String>> {
        let end = Instant::now() + wait;
        let mut s = lock(&self.st);
        loop {
            if let Some(o) = &s.outcome {
                return Some(o.clone());
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            s = self
                .cv
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// After the drop (`taken`: the drop target read the URI list and finished): did the
    /// files go where it asked? A target that read the object calls it within `call_wait`;
    /// then the copy decides (waited for at most `outcome_wait`). No call: `taken` stands.
    pub fn taken_after_drop(
        &self,
        taken: bool,
        call_wait: Duration,
        outcome_wait: Duration,
    ) -> bool {
        let end = Instant::now() + call_wait;
        let mut s = lock(&self.st);
        while !s.claimed {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                log::info!("Flow: pickup: the drop target read the KDE offer but didn't call it");
                return taken;
            }
            s = self
                .cv
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(s);
        match self.wait_outcome(outcome_wait) {
            Some(r) => r.is_ok(),
            None => {
                log::info!("Flow: pickup: the copy on KDE's drop is taking too long");
                false
            }
        }
    }

    /// Runs the call on a thread of its own: the answer (`None`: still going on after
    /// `reply_wait`) comes from another.
    fn start(self: &Arc<Self>, destination: String, reply_wait: Duration) -> Answer {
        let answer = Answer::default();
        let me = self.clone();
        let work = std::thread::Builder::new()
            .name("flow-ark-copy".into())
            .spawn(move || {
                let _ = me.extract(&destination, FILES_WAIT);
            });
        if work.is_err() {
            let e = "couldn't start the copy".to_string();
            lock(&self.st).outcome = Some(Err(e.clone()));
            self.cv.notify_all();
            answer.give(Some(Err(e)));
            return answer;
        }
        let (me, a) = (self.clone(), answer.clone());
        let replier = std::thread::Builder::new()
            .name("flow-ark-reply".into())
            .spawn(move || a.give(me.wait_outcome(reply_wait)));
        if replier.is_err() {
            answer.give(None);
        }
        answer
    }
}

/// The answer a thread hands to the D-Bus method waiting for it (without blocking the
/// connection's executor).
#[derive(Clone, Default)]
struct Answer(Arc<Mutex<AnswerSlot>>);

type AnswerSlot = (Option<Option<Result<usize, String>>>, Option<Waker>);

impl Answer {
    fn give(&self, v: Option<Result<usize, String>>) {
        let mut g = lock(&self.0);
        g.0 = Some(v);
        if let Some(w) = g.1.take() {
            w.wake();
        }
    }
}

impl Future for Answer {
    type Output = Option<Result<usize, String>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut g = lock(&self.0);
        match g.0.take() {
            Some(v) => Poll::Ready(v),
            None => {
                g.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// `org.kde.ark.DndExtract` at the drag's own object path ([`ArkDrop::path`]).
pub struct DndExtract(pub Arc<ArkDrop>);

#[zbus::interface(name = "org.kde.ark.DndExtract")]
impl DndExtract {
    /// Copies the drag's files into `destination` (see the module's documentation).
    #[zbus(name = "extractSelectedFilesTo")]
    async fn extract_selected_files_to(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        destination: String,
    ) -> zbus::fdo::Result<()> {
        if !same_user(conn, &hdr).await {
            log::warn!("Flow: pickup: a KDE drop call from another user refused");
            return Err(zbus::fdo::Error::AccessDenied(
                "not a process of this user".into(),
            ));
        }
        if !self.0.claim() {
            return Err(zbus::fdo::Error::AccessDenied(
                "this drop was already taken".into(),
            ));
        }
        // One call per drag: the object goes now.
        let _ = server.remove::<Self, _>(self.0.path()).await;
        match self.0.start(destination, REPLY_WAIT).await {
            Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(zbus::fdo::Error::Failed(e)),
            None => Ok(()), // still copying (or waiting for the files): it goes on
        }
    }
}

/// Is the caller a connection of this user (asked to the bus)?
async fn same_user(conn: &zbus::Connection, hdr: &zbus::message::Header<'_>) -> bool {
    let Some(sender) = hdr.sender() else {
        return false;
    };
    let uid = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetConnectionUnixUser",
            &(sender.as_str(),),
        )
        .await
        .ok()
        .and_then(|m| m.body().deserialize::<u32>().ok());
    uid == Some(rustix::process::getuid().as_raw())
}

// ================================================================ the copy
/// Where the files go: an existing local folder this user can write to, canonical, opened
/// once (everything is created relative to it).
struct Destination {
    path: PathBuf,
    fd: OwnedFd,
}

fn dir_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

impl Destination {
    fn open(s: &str) -> Result<Self, String> {
        const NOT_LOCAL: &str = "the destination isn't a local folder";
        let raw = if s.starts_with("file:") {
            let mut p = crate::drag::parse_drag_uris(s.as_bytes());
            if p.len() != 1 {
                return Err(NOT_LOCAL.into());
            }
            p.remove(0)
        } else {
            PathBuf::from(s)
        };
        if !raw.is_absolute() {
            return Err(NOT_LOCAL.into());
        }
        let path = std::fs::canonicalize(&raw)
            .map_err(|_| "the destination folder doesn't exist".to_string())?;
        let fd = rustix::fs::open(&path, dir_flags(), Mode::empty())
            .map_err(|_| "the destination isn't a folder".to_string())?;
        rustix::fs::access(&path, Access::WRITE_OK | Access::EXEC_OK)
            .map_err(|_| "the destination folder can't be written to".to_string())?;
        Ok(Self { path, fd })
    }
}

/// A received item, or anything inside one, for the error messages: its name only.
fn named(name: &OsStr, e: impl std::fmt::Display) -> String {
    format!("{}: {e}", name.to_string_lossy())
}

/// Copies the received `items` (top-level, all in one staging folder) into `dest`: see the
/// module's documentation. All or nothing: on an error, what was created is removed.
fn copy_items(items: &[PathBuf], dest: &Destination) -> Result<usize, String> {
    let folder = items
        .first()
        .and_then(|p| p.parent())
        .ok_or_else(|| "nothing to copy".to_string())?;
    let mut names: Vec<&OsStr> = Vec::new();
    for p in items {
        let name = p.file_name().filter(|_| p.parent() == Some(folder));
        match name {
            Some(n) if n != "." && n != ".." && !n.as_bytes().contains(&b'/') => names.push(n),
            _ => return Err("the received items aren't in one folder".into()),
        }
    }
    let src = rustix::fs::open(folder, dir_flags(), Mode::empty())
        .map_err(|e| format!("the received files can't be read ({e})"))?;
    let staged = std::fs::canonicalize(folder)
        .map_err(|e| format!("the received files can't be read ({e})"))?;
    if dest.path.starts_with(&staged) {
        return Err("the destination is inside the dragged files".into());
    }
    let mut created: Vec<OsString> = Vec::new();
    let mut result = Ok(());
    for name in names {
        result = copy_top(&src, name, &dest.fd, &mut created);
        if result.is_err() {
            break;
        }
    }
    match result {
        Ok(()) if created.is_empty() => Err("nothing could be copied".into()),
        Ok(()) => Ok(created.len()),
        Err(e) => {
            for c in &created {
                let _ = remove_tree(&dest.fd, c, 0);
            }
            Err(e)
        }
    }
}

/// One received item into the destination, under a free name.
fn copy_top(
    src: &OwnedFd,
    name: &OsStr,
    dest: &OwnedFd,
    created: &mut Vec<OsString>,
) -> Result<(), String> {
    let st =
        rustix::fs::statat(src, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|e| named(name, e))?;
    match FileType::from_raw_mode(st.st_mode) {
        FileType::Directory => {
            let s = rustix::fs::openat(src, name, dir_flags(), Mode::empty())
                .map_err(|e| named(name, e))?;
            let (new, ()) = free_name(name, |n| {
                rustix::fs::mkdirat(dest, n, Mode::from_raw_mode(0o700))
            })?;
            created.push(new.clone());
            let d = rustix::fs::openat(dest, &new, dir_flags(), Mode::empty())
                .map_err(|e| named(name, e))?;
            copy_dir(&s, &d, 1)?;
            finish_dir(&s, &d, dest, &new).map_err(|e| named(name, e))
        }
        FileType::RegularFile => {
            let (s, sst) = open_file(src, name).map_err(|e| named(name, e))?;
            let (new, d) = free_name(name, |n| new_file(dest, n))?;
            created.push(new);
            copy_file(s, d, &sst).map_err(|e| named(name, e))
        }
        _ => {
            log::info!("Flow: pickup: a received item that isn't a file or folder was skipped");
            Ok(())
        }
    }
}

/// `make(name)`, or `make("name (2)")`… while the name is taken (nothing is overwritten).
fn free_name<T>(
    name: &OsStr,
    mut make: impl FnMut(&OsStr) -> rustix::io::Result<T>,
) -> Result<(OsString, T), String> {
    match make(name) {
        Ok(t) => return Ok((name.to_os_string(), t)),
        Err(Errno::EXIST) => {}
        Err(e) => return Err(named(name, e)),
    }
    let base = name.to_string_lossy();
    for n in 2..10_000 {
        let cand = OsString::from(crate::names::numbered(&base, n));
        match make(&cand) {
            Ok(t) => return Ok((cand, t)),
            Err(Errno::EXIST) => continue,
            Err(e) => return Err(named(name, e)),
        }
    }
    Err(named(name, "no free name for it in the destination"))
}

/// A regular file of the staging folder, opened without following a link (and without
/// blocking on anything that isn't a regular file), with its metadata.
fn open_file(dir: &OwnedFd, name: &OsStr) -> rustix::io::Result<(File, Stat)> {
    let fd = rustix::fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let st = rustix::fs::fstat(&fd)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
        return Err(Errno::INVAL);
    }
    Ok((File::from(fd), st))
}

/// A new file (never an existing one, never through a link), private until it is filled.
fn new_file(dir: &OwnedFd, name: &OsStr) -> rustix::io::Result<File> {
    let fd = rustix::fs::openat(
        dir,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    Ok(File::from(fd))
}

fn times(st: &Stat) -> Timestamps {
    Timestamps {
        last_access: Timespec {
            tv_sec: st.st_atime as _,
            tv_nsec: st.st_atime_nsec as _,
        },
        last_modification: Timespec {
            tv_sec: st.st_mtime as _,
            tv_nsec: st.st_mtime_nsec as _,
        },
    }
}

/// The contents, then the staged mode without execute bits and the times.
fn copy_file(mut src: File, mut dst: File, st: &Stat) -> std::io::Result<()> {
    std::io::copy(&mut src, &mut dst)?;
    rustix::fs::fchmod(&dst, Mode::from_raw_mode(st.st_mode & 0o666))?;
    rustix::fs::futimens(&dst, &times(st))?;
    Ok(())
}

/// A copied folder: the staged mode (owner always rwx) and times, once it is filled.
fn finish_dir(src: &OwnedFd, dst: &OwnedFd, parent: &OwnedFd, name: &OsStr) -> std::io::Result<()> {
    let st = rustix::fs::fstat(src)?;
    rustix::fs::fchmod(dst, Mode::from_raw_mode((st.st_mode & 0o777) | 0o700))?;
    rustix::fs::utimensat(parent, name, &times(&st), AtFlags::SYMLINK_NOFOLLOW)?;
    Ok(())
}

/// The contents of a staged folder into a new one of ours (which nothing else is in).
fn copy_dir(src: &OwnedFd, dst: &OwnedFd, depth: usize) -> Result<(), String> {
    if depth > crate::MAX_DEPTH + 1 {
        return Err("the folders are nested too deep".into());
    }
    for name in entries(src).map_err(|e| format!("a received folder can't be read ({e})"))? {
        let st = rustix::fs::statat(src, &name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| named(&name, e))?;
        match FileType::from_raw_mode(st.st_mode) {
            FileType::Directory => {
                let s = rustix::fs::openat(src, &name, dir_flags(), Mode::empty())
                    .map_err(|e| named(&name, e))?;
                rustix::fs::mkdirat(dst, &name, Mode::from_raw_mode(0o700))
                    .map_err(|e| named(&name, e))?;
                let d = rustix::fs::openat(dst, &name, dir_flags(), Mode::empty())
                    .map_err(|e| named(&name, e))?;
                copy_dir(&s, &d, depth + 1)?;
                finish_dir(&s, &d, dst, &name).map_err(|e| named(&name, e))?;
            }
            FileType::RegularFile => {
                let (s, sst) = open_file(src, &name).map_err(|e| named(&name, e))?;
                let d = new_file(dst, &name).map_err(|e| named(&name, e))?;
                copy_file(s, d, &sst).map_err(|e| named(&name, e))?;
            }
            _ => log::info!("Flow: pickup: something that isn't a file or folder was skipped"),
        }
    }
    Ok(())
}

/// The names in a folder (without `.` and `..`).
fn entries(dir: &OwnedFd) -> rustix::io::Result<Vec<OsString>> {
    let mut out = Vec::new();
    for e in rustix::fs::Dir::read_from(dir.as_fd())? {
        let e = e?;
        let n = e.file_name().to_bytes();
        if n != b"." && n != b".." {
            out.push(OsStr::from_bytes(n).to_os_string());
        }
    }
    Ok(out)
}

/// Removes what a failed copy created (relative to the folder, never through a link).
fn remove_tree(dir: &OwnedFd, name: &OsStr, depth: usize) -> rustix::io::Result<()> {
    let st = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(st.st_mode) == FileType::Directory && depth <= crate::MAX_DEPTH + 2 {
        let sub = rustix::fs::openat(dir, name, dir_flags(), Mode::empty())?;
        for n in entries(&sub)? {
            let _ = remove_tree(&sub, &n, depth + 1);
        }
        rustix::fs::unlinkat(dir, name, AtFlags::REMOVEDIR)
    } else {
        rustix::fs::unlinkat(dir, name, AtFlags::empty())
    }
}

#[cfg(test)]
mod tests;
