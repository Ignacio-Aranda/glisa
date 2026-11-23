//! Persistent per-device settings: `config.json` in the config directory
//! (`~/.config/glisa/config.json` on Linux: the same format as the Python version, whose
//! file is imported on the first start, see `glisa_common::paths::migrate_legacy`).
//!
//! ```json
//! {"version": 1, "app": {}, "devices": {"B031:LIFT": {"buttons": {"0x0053": {…}}, "dpi": 1600,
//!   "display_name": "Desk"}}}
//! ```
//!
//! The file is user-editable, so everything read from it is type-checked where used.
//! Writes are atomic and private (0600, directory 0700).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};

use crate::fsutil::{read_limited, write_private_atomic};

/// A settings file bigger than this is ignored (it is ours, and tiny).
const MAX_CONFIG_BYTES: u64 = 16 << 20;

pub struct Config {
    path: PathBuf,
    data: Mutex<Map<String, Value>>,
}

fn defaults() -> Map<String, Value> {
    let Value::Object(m) = json!({"version": 1, "devices": {}, "app": {}}) else {
        unreachable!()
    };
    m
}

/// `config.json` in the platform's config directory.
pub fn default_path() -> PathBuf {
    glisa_common::paths::config_dir().join("config.json")
}

fn button_key(cid: u16) -> String {
    format!("{cid:#06x}")
}

impl Config {
    /// Loads `path` (default: [`default_path`]). A missing or broken file means "no
    /// settings yet".
    pub fn new(path: Option<PathBuf>) -> Self {
        let c = Config {
            path: path.unwrap_or_else(default_path),
            data: Mutex::new(defaults()),
        };
        c.load();
        c
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Map<String, Value>> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn load(&self) {
        let Some(bytes) = read_limited(&self.path, MAX_CONFIG_BYTES) else {
            return;
        };
        let Ok(Value::Object(loaded)) = serde_json::from_slice::<Value>(&bytes) else {
            return;
        };
        let mut data = self.lock();
        for (k, v) in loaded {
            data.insert(k, v);
        }
        for key in ["devices", "app"] {
            if !data.get(key).is_some_and(Value::is_object) {
                data.insert(key.into(), json!({}));
            }
        }
    }

    fn save_locked(&self, data: &Map<String, Value>) -> io::Result<()> {
        let mut text = serde_json::to_string_pretty(data).map_err(io::Error::other)?;
        text.push('\n');
        write_private_atomic(&self.path, text.as_bytes())
    }

    pub fn save(&self) -> io::Result<()> {
        let data = self.lock();
        self.save_locked(&data)
    }

    /// The whole document (tests, diagnostics).
    pub fn snapshot(&self) -> Value {
        Value::Object(self.lock().clone())
    }

    /// The app-wide section (`"app"`).
    pub fn app(&self) -> Map<String, Value> {
        self.lock()
            .get("app")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    fn devices(data: &mut Map<String, Value>) -> &mut Map<String, Value> {
        if !data.get("devices").is_some_and(Value::is_object) {
            data.insert("devices".into(), json!({}));
        }
        data.get_mut("devices")
            .and_then(Value::as_object_mut)
            .expect("devices is an object")
    }

    /// The device's entry, created with `"buttons": {}` if needed.
    fn device_entry<'a>(data: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
        let devices = Self::devices(data);
        if !devices.get(key).is_some_and(Value::is_object) {
            devices.insert(key.into(), json!({}));
        }
        let entry = devices
            .get_mut(key)
            .and_then(Value::as_object_mut)
            .expect("entry is an object");
        if !entry.get("buttons").is_some_and(Value::is_object) {
            entry.insert("buttons".into(), json!({}));
        }
        entry
    }

    /// A copy of the device's settings, with `buttons` always an object. Reading never
    /// creates an entry.
    pub fn device_copy(&self, key: &str) -> Map<String, Value> {
        let data = self.lock();
        let mut out = data
            .get("devices")
            .and_then(|d| d.get(key))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if !out.get("buttons").is_some_and(Value::is_object) {
            out.insert("buttons".into(), json!({}));
        }
        out
    }

    pub fn update_device(&self, key: &str, values: Map<String, Value>) -> io::Result<()> {
        let mut data = self.lock();
        Self::device_entry(&mut data, key).extend(values);
        self.save_locked(&data)
    }

    /// Stores what control `cid` does; `None`, `{}` or `{"type": "default"}` removes it.
    pub fn set_button(&self, key: &str, cid: u16, action: Option<&Value>) -> io::Result<()> {
        let mut data = self.lock();
        let entry = Self::device_entry(&mut data, key);
        let buttons = entry
            .get_mut("buttons")
            .and_then(Value::as_object_mut)
            .expect("buttons is an object");
        let empty = match action {
            None | Some(Value::Null) => true,
            Some(Value::Object(m)) => {
                m.is_empty() || m.get("type").and_then(Value::as_str) == Some("default")
            }
            Some(_) => false,
        };
        if empty {
            buttons.remove(&button_key(cid));
        } else if let Some(a) = action {
            buttons.insert(button_key(cid), a.clone());
        }
        self.save_locked(&data)
    }

    /// The name the user gave the device ("" when none). Never creates an entry.
    pub fn display_name(&self, key: &str) -> String {
        self.lock()
            .get("devices")
            .and_then(|d| d.get(key))
            .and_then(|e| e.get("display_name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Stores a (clean) name; `None` or "" removes it.
    pub fn set_display_name(&self, key: &str, name: Option<&str>) -> io::Result<()> {
        let mut data = self.lock();
        match name.filter(|n| !n.is_empty()) {
            Some(n) => {
                Self::device_entry(&mut data, key).insert("display_name".into(), json!(n));
            }
            None => {
                let Some(entry) = Self::devices(&mut data)
                    .get_mut(key)
                    .and_then(Value::as_object_mut)
                else {
                    return Ok(());
                };
                if entry.remove("display_name").is_none() {
                    return Ok(());
                }
            }
        }
        self.save_locked(&data)
    }

    /// Forgets the device's settings (buttons, pointer…); its name stays.
    pub fn reset_device(&self, key: &str) -> io::Result<()> {
        let name = self.display_name(key);
        let mut data = self.lock();
        Self::devices(&mut data).remove(key);
        if !name.is_empty() {
            Self::device_entry(&mut data, key).insert("display_name".into(), json!(name));
        }
        self.save_locked(&data)
    }

    /// Forgets everything stored for the device, its name included.
    pub fn forget_device(&self, key: &str) -> io::Result<()> {
        let mut data = self.lock();
        if Self::devices(&mut data).remove(key).is_some() {
            self.save_locked(&data)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lo-config-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("glisa").join("config.json")
    }

    #[test]
    fn round_trip_and_python_format() {
        let path = tmp("rt");
        let c = Config::new(Some(path.clone()));
        let mut v = Map::new();
        v.insert("dpi".into(), json!(1600));
        c.update_device("B031:LIFT", v).unwrap();
        c.set_button(
            "B031:LIFT",
            0x53,
            Some(&json!({"type": "keys", "combo": "Ctrl+C"})),
        )
        .unwrap();
        let again = Config::new(Some(path.clone()));
        let d = again.device_copy("B031:LIFT");
        assert_eq!(d["dpi"], json!(1600));
        assert_eq!(d["buttons"]["0x0053"]["combo"], json!("Ctrl+C"));
        c.set_button("B031:LIFT", 0x53, Some(&json!({"type": "default"})))
            .unwrap();
        assert_eq!(c.device_copy("B031:LIFT")["buttons"], json!({}));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let dmode = std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dmode, 0o700);
        }
        std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn reads_python_files_and_survives_garbage() {
        let path = tmp("py");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version": 1, "devices": {"B031:LIFT": {"buttons": {"0x0053": {"type": "keys", "combo": "Ctrl+Z"}}, "display_name": "Desk", "dpi": 2400}}, "app": {"theme": "dark"}}"#,
        )
        .unwrap();
        let c = Config::new(Some(path.clone()));
        assert_eq!(c.display_name("B031:LIFT"), "Desk");
        assert_eq!(c.app()["theme"], json!("dark"));
        std::fs::write(&path, r#"{"devices": [1, 2], "app": 3}"#).unwrap();
        let c = Config::new(Some(path.clone()));
        assert_eq!(
            c.device_copy("x"),
            json!({"buttons": {}}).as_object().unwrap().clone()
        );
        std::fs::write(&path, "not json").unwrap();
        let c = Config::new(Some(path.clone()));
        assert_eq!(c.snapshot()["version"], json!(1));
        std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn names_reset_and_forget() {
        let path = tmp("names");
        let c = Config::new(Some(path.clone()));
        assert_eq!(c.display_name("K"), "");
        c.set_display_name("K", None).unwrap();
        assert!(
            !path.exists(),
            "removing a name that isn't there writes nothing"
        );
        c.set_display_name("K", Some("Desk")).unwrap();
        let mut v = Map::new();
        v.insert("dpi".into(), json!(800));
        c.update_device("K", v).unwrap();
        c.reset_device("K").unwrap();
        assert_eq!(
            c.snapshot()["devices"]["K"],
            json!({"buttons": {}, "display_name": "Desk"})
        );
        c.forget_device("K").unwrap();
        assert_eq!(c.snapshot()["devices"], json!({}));
        std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }
}
