//! Optional official product photos for the Devices cards.
//!
//! Off by default (*App settings → Show product photos*). `data/photos.json` (embedded at
//! build time) maps a model key to the URL of a product photo on Logitech's public
//! website; the repository holds URLs only, never images. When the setting is on, the
//! photo is downloaded on this computer the first time it is needed and cached.
//!
//! Safety: HTTPS only, hosts on an allowlist, no redirects, at most 5 MB, PNG, JPEG or
//! WebP checked by signature and decoded dimensions, model keys restricted to
//! `[a-z0-9-]`, cache files private. Failures are remembered for the session so a
//! missing photo is not fetched again and again.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use glisa_common::{APP_NAME, VERSION, paths};
use image::{ImageFormat, ImageReader, Limits};
use serde::Deserialize;

use crate::fsutil::{read_untrusted, write_private};

const TABLE: &str = include_str!(concat!(env!("OUT_DIR"), "/photos.json"));
pub const ALLOWED_HOSTS: &[&str] = &["resource.logitech.com"];
pub const MAX_BYTES: u64 = 5 << 20;
const MAX_SIDE: u32 = 6000;

#[derive(Debug, Clone, Deserialize)]
struct Entry {
    url: String,
}

pub struct Photos {
    table: HashMap<String, Entry>,
    failed: Mutex<HashSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Png,
    Jpeg,
    Webp,
}

impl Kind {
    fn sniff(data: &[u8]) -> Option<Self> {
        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Kind::Png)
        } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Kind::Jpeg)
        } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
            Some(Kind::Webp)
        } else {
            None
        }
    }
    fn ext(self) -> &'static str {
        match self {
            Kind::Png => "png",
            Kind::Jpeg => "jpg",
            Kind::Webp => "webp",
        }
    }
    fn mime(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Jpeg => "image/jpeg",
            Kind::Webp => "image/webp",
        }
    }
    fn format(self) -> ImageFormat {
        match self {
            Kind::Png => ImageFormat::Png,
            Kind::Jpeg => ImageFormat::Jpeg,
            Kind::Webp => ImageFormat::WebP,
        }
    }
}

/// Model keys are slugs; anything else is refused before it gets near a path.
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `https://<allowed host>/<path>` with no user info, port, or odd characters.
pub fn allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    rest.len() > host.len()
        && ALLOWED_HOSTS.contains(&host)
        && url.len() <= 2048
        && url.bytes().all(|b| b.is_ascii_graphic())
}

/// Checks downloaded or cached bytes: known format, sane decoded dimensions.
fn check_image(data: &[u8]) -> Option<Kind> {
    let kind = Kind::sniff(data)?;
    let mut reader = ImageReader::with_format(Cursor::new(data), kind.format());
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    reader.limits(limits);
    let (w, h) = reader.into_dimensions().ok()?;
    (w > 0 && h > 0 && w <= MAX_SIDE && h <= MAX_SIDE).then_some(kind)
}

fn data_url(kind: Kind, data: &[u8]) -> String {
    format!(
        "data:{};base64,{}",
        kind.mime(),
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

impl Photos {
    pub fn load() -> Self {
        let table: HashMap<String, Entry> = serde_json::from_str(TABLE).unwrap_or_else(|e| {
            log::warn!("data/photos.json is not valid: {e}");
            HashMap::new()
        });
        let table = table
            .into_iter()
            .filter(|(k, e)| valid_key(k) && allowed_url(&e.url))
            .collect();
        Self {
            table,
            failed: Mutex::new(HashSet::new()),
        }
    }

    fn cache_dir() -> PathBuf {
        paths::cache_dir().join("photos")
    }

    /// The photo of model `key` as a `data:` URL: from the cache, else downloaded now
    /// (blocking; call off the main thread).
    pub fn get(&self, key: &str) -> Option<String> {
        if !valid_key(key) {
            return None;
        }
        let entry = self.table.get(key)?;
        for kind in [Kind::Webp, Kind::Png, Kind::Jpeg] {
            let path = Self::cache_dir().join(format!("{key}.{}", kind.ext()));
            if let Ok(data) = read_untrusted(&path, MAX_BYTES) {
                if check_image(&data) == Some(kind) {
                    return Some(data_url(kind, &data));
                }
            }
        }
        if self
            .failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(key)
        {
            return None;
        }
        match download(&entry.url) {
            Ok((kind, data)) => {
                let path = Self::cache_dir().join(format!("{key}.{}", kind.ext()));
                if let Err(e) = write_private(&path, &data) {
                    log::warn!("can't cache the photo of {key}: {e}");
                }
                Some(data_url(kind, &data))
            }
            Err(e) => {
                log::info!("no product photo for {key}: {e}");
                self.failed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key.to_string());
                None
            }
        }
    }

    /// Forgets the cached photos (the setting was turned off).
    pub fn clear_cache(&self) {
        let _ = std::fs::remove_dir_all(Self::cache_dir());
        self.failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

fn download(url: &str) -> Result<(Kind, Vec<u8>), String> {
    if !allowed_url(url) {
        return Err("URL not allowed".into());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(format!("{APP_NAME}/{VERSION}"))
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let data = resp
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    let kind = check_image(&data).ok_or("not a PNG, JPEG or WebP image of a sane size")?;
    Ok((kind, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_allowlisted_https_urls() {
        assert!(allowed_url(
            "https://resource.logitech.com/w_800/content/dam/lift.png"
        ));
        assert!(!allowed_url("http://resource.logitech.com/x.png"));
        assert!(!allowed_url(
            "https://resource.logitech.com.evil.example/x.png"
        ));
        assert!(!allowed_url("https://user@resource.logitech.com/x.png"));
        assert!(!allowed_url("https://resource.logitech.com:8443/x.png"));
        assert!(!allowed_url("https://resource.logitech.com"));
        assert!(!allowed_url("https://resource.logitech.com/a b.png"));
    }

    #[test]
    fn keys_are_slugs() {
        assert!(valid_key("mx-master-3s"));
        assert!(!valid_key("../etc/passwd"));
        assert!(!valid_key("MX"));
        assert!(!valid_key(""));
    }

    #[test]
    fn images_are_sniffed_and_checked() {
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        assert_eq!(check_image(&png), Some(Kind::Png));
        assert_eq!(check_image(b"<html>not found</html>"), None);
        let mut huge = png.clone();
        huge[16..20].copy_from_slice(&50_000u32.to_be_bytes());
        assert_eq!(check_image(&huge), None);
    }
}
