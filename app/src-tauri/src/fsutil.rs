//! Private files: written atomically, readable only by this user (0600 in a 0700
//! directory on Unix; per-user profile directories on Windows), and untrusted files read
//! with a size cap.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

/// Creates `dir` (and parents) closed to other users.
pub fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let meta = fs::symlink_metadata(dir)?;
        if !meta.is_dir() {
            return Err(io::Error::other(format!(
                "{} is not a directory",
                dir.display()
            )));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Writes `data` to `path` through a temporary file in the same directory and a rename,
/// so readers never see half a file. The file is 0600 on Unix.
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("no parent directory"))?;
    private_dir(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600).custom_flags(libc_o_nofollow());
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(unix)]
fn libc_o_nofollow() -> i32 {
    // O_NOFOLLOW: never write through a symlink planted at the temporary name.
    #[cfg(target_os = "linux")]
    {
        0o400000
    }
    #[cfg(not(target_os = "linux"))]
    {
        0x0100
    }
}

/// Reads a file the user picked (or anything else we don't trust): a regular file of at
/// most `limit` bytes. `Err(None)` = too large.
pub fn read_untrusted(path: &Path, limit: u64) -> Result<Vec<u8>, Option<io::Error>> {
    let f = fs::File::open(path).map_err(Some)?;
    let meta = f.metadata().map_err(Some)?;
    if !meta.is_file() {
        return Err(Some(io::Error::other("not a regular file")));
    }
    if meta.len() > limit {
        return Err(None);
    }
    let mut data = Vec::with_capacity(meta.len() as usize);
    f.take(limit + 1).read_to_end(&mut data).map_err(Some)?;
    if data.len() as u64 > limit {
        return Err(None);
    }
    Ok(data)
}

/// Lowercase hex of SHA-256, used to turn ids into file names.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
