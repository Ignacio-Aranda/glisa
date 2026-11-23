//! Private files: directories 0700, files 0600, written atomically (temporary file in
//! the same directory, fsync, rename), so a crash never leaves half a settings file and
//! nobody else can read what Glisa stores.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Creates `dir` and its missing parents, closed to other users (Unix 0700; Windows
/// per-user profile directories are already private).
pub fn create_private_dir_all(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Replaces `path` with `data`: a new 0600 file, never a partial one.
pub fn write_private_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_private_dir_all(dir)?;
    let stem = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(
        ".{stem}.{}.{nanos:x}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Reads at most `limit` bytes of a regular file (`None` when it is missing, is not a
/// regular file, or is bigger): untrusted files are never read whole into memory, and
/// a FIFO never blocks us.
pub fn read_limited(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > limit {
        return None;
    }
    let f = File::open(path).ok()?;
    let mut out = Vec::new();
    f.take(limit + 1).read_to_end(&mut out).ok()?;
    (out.len() as u64 <= limit).then_some(out)
}

/// "Permission denied" for an I/O error (without Rust's " (os error 13)").
pub fn error_text(e: &io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_atomic_write() {
        let base = std::env::temp_dir().join(format!("lo-fsutil-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let path = base.join("a").join("b").join("c.json");
        write_private_atomic(&path, b"one").unwrap();
        write_private_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&base.join("a")), 0o700);
            assert_eq!(mode(&base.join("a").join("b")), 0o700);
        }
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
        assert_eq!(read_limited(&path, 3), Some(b"two".to_vec()));
        assert_eq!(read_limited(&path, 2), None);
        assert_eq!(read_limited(&base, 100), None);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn error_texts() {
        let e = io::Error::from_raw_os_error(2);
        assert!(!error_text(&e).contains("os error"));
    }
}
