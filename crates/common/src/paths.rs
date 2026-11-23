//! Where Glisa keeps things on each OS.
//!
//! | | Linux | Windows | macOS |
//! |---|---|---|---|
//! | config | `$XDG_CONFIG_HOME/glisa` | `%APPDATA%\Glisa` | `~/Library/Application Support/Glisa` |
//! | cache | `$XDG_CACHE_HOME/glisa` | `%LOCALAPPDATA%\Glisa\cache` | `~/Library/Caches/Glisa` |
//! | runtime (IPC) | `$XDG_RUNTIME_DIR` (or a private `/tmp/glisa-<uid>`) | named pipe | private dir under `$TMPDIR` |
//!
//! On Linux the Python version (LinOptions) kept its settings and Flow identity in
//! `~/.config/linoptions`; [`migrate_legacy`] copies them over on the first start.

use std::io;
use std::path::PathBuf;

use crate::{APP_DIR, APP_NAME};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        env_dir("APPDATA")
            .unwrap_or_else(|| home().join("AppData").join("Roaming"))
            .join(APP_NAME)
    } else if cfg!(target_os = "macos") {
        home()
            .join("Library")
            .join("Application Support")
            .join(APP_NAME)
    } else {
        env_dir("XDG_CONFIG_HOME")
            .unwrap_or_else(|| home().join(".config"))
            .join(APP_DIR)
    }
}

pub fn cache_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        env_dir("LOCALAPPDATA")
            .unwrap_or_else(|| home().join("AppData").join("Local"))
            .join(APP_NAME)
            .join("cache")
    } else if cfg!(target_os = "macos") {
        home().join("Library").join("Caches").join(APP_NAME)
    } else {
        env_dir("XDG_CACHE_HOME")
            .unwrap_or_else(|| home().join(".cache"))
            .join(APP_DIR)
    }
}

/// Private per-user directory for sockets and other runtime files (Unix only).
///
/// Linux: `$XDG_RUNTIME_DIR` (created by logind, owned by the user, 0700). Without it,
/// `/tmp/glisa-<uid>`, used only if it is a real directory owned by us and closed
/// to everybody else: anyone could have created it first. macOS: `$TMPDIR` is already
/// per user (`/var/folders/…`, 0700); we add our own 0700 directory inside it with the
/// same checks. Never falls back to something shared.
#[cfg(unix)]
pub fn runtime_dir() -> io::Result<PathBuf> {
    if cfg!(target_os = "linux") {
        if let Some(base) = env_dir("XDG_RUNTIME_DIR") {
            std::fs::create_dir_all(&base)?;
            return Ok(base);
        }
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        private_dir(PathBuf::from("/tmp").join(format!("{APP_DIR}-{uid}")))
    } else {
        let base = env_dir("TMPDIR").unwrap_or_else(|| PathBuf::from("/tmp"));
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        private_dir(base.join(format!("{APP_DIR}-{uid}")))
    }
}

#[cfg(unix)]
fn private_dir(path: PathBuf) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match std::fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(&path)?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is not a private directory of this user (did another user create it?)",
                path.display()
            ),
        ));
    }
    Ok(path)
}

/// Where the service listens for `glisa-ctl`: a Unix socket path, or on Windows a
/// named pipe name (`\\.\pipe\glisa-<suffix>`; the service appends the user's SID).
pub fn ipc_endpoint() -> io::Result<String> {
    #[cfg(unix)]
    {
        Ok(runtime_dir()?
            .join(format!("{APP_DIR}.sock"))
            .to_string_lossy()
            .into_owned())
    }
    #[cfg(not(unix))]
    {
        Ok(format!(r"\\.\pipe\{APP_DIR}"))
    }
}

/// The Python version's (LinOptions) settings directory on Linux.
pub fn legacy_config_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    Some(
        env_dir("XDG_CONFIG_HOME")
            .unwrap_or_else(|| home().join(".config"))
            .join("linoptions"),
    )
}

/// First start after the rename: copies the old settings (`config.json`) and the Flow
/// identity and pairings (`flow/`) into the new settings directory, so devices keep their
/// buttons and paired computers stay paired. Runs only while the new directory doesn't
/// exist; the old one is left untouched (the Python version may still be installed).
/// Returns whether anything was copied.
pub fn migrate_legacy() -> io::Result<bool> {
    let Some(old) = legacy_config_dir() else {
        return Ok(false);
    };
    let new = config_dir();
    if new.exists() || !old.is_dir() {
        return Ok(false);
    }
    let mut copied = false;
    create_private(&new)?;
    if old.join("config.json").is_file() {
        copy_file(&old.join("config.json"), &new.join("config.json"))?;
        copied = true;
    }
    let flow = old.join("flow");
    if flow.is_dir() && !std::fs::symlink_metadata(&flow)?.file_type().is_symlink() {
        create_private(&new.join("flow"))?;
        for entry in std::fs::read_dir(&flow)? {
            let entry = entry?;
            // Regular files only (key, certificate, flow.json): no links, no subfolders.
            if entry.file_type()?.is_file() {
                copy_file(&entry.path(), &new.join("flow").join(entry.file_name()))?;
                copied = true;
            }
        }
    }
    Ok(copied)
}

fn create_private(dir: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Copies a file keeping it private (0600 on Unix: it may be a private key).
fn copy_file(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    std::fs::copy(from, to)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn legacy_settings_and_pairings_are_imported_once() {
        let base = std::env::temp_dir().join(format!("glisa-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join("linoptions");
        std::fs::create_dir_all(old.join("flow")).unwrap();
        std::fs::write(old.join("config.json"), "{\"version\": 1}").unwrap();
        std::fs::write(old.join("flow").join("key.pem"), "KEY").unwrap();
        std::fs::write(old.join("gui.conf"), "ignored").unwrap();
        // SAFETY (env): this is the only test in the crate that touches XDG_CONFIG_HOME.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &base) };
        assert!(migrate_legacy().unwrap());
        let new = config_dir();
        assert_eq!(
            std::fs::read_to_string(new.join("config.json")).unwrap(),
            "{\"version\": 1}"
        );
        assert_eq!(
            std::fs::read_to_string(new.join("flow").join("key.pem")).unwrap(),
            "KEY"
        );
        assert!(!new.join("gui.conf").exists());
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(new.join("flow").join("key.pem"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Second start: nothing happens, the old directory is untouched.
        std::fs::write(new.join("config.json"), "changed").unwrap();
        assert!(!migrate_legacy().unwrap());
        assert_eq!(
            std::fs::read_to_string(new.join("config.json")).unwrap(),
            "changed"
        );
        assert!(old.join("config.json").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
