//! A picture of the user's choosing for a device: from a file or an https link (their
//! own photo with the background removed, a product photo they pick…). It shows on the
//! device's card in *Devices* and, if the user turns it on, on the device pages too,
//! with the buttons where the user placed them (the *layout*).
//!
//! The picture is untrusted: it is read with a size cap, decoded with pixel and memory
//! limits by a memory-safe decoder (PNG, JPEG, WebP, GIF, BMP only), reduced to fit
//! 1024 × 1024 and re-encoded as a fresh PNG — nothing from the original but its pixels
//! is kept (no metadata, no extra chunks). Links: https only, public addresses only, at
//! most 3 redirects (each checked), same size cap. Stored privately in
//! `<config dir>/pictures/<sha256(device id)>.png` (+ `.json` for the layout).

use std::collections::BTreeMap;
use std::io::Cursor;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use glisa_common::{APP_NAME, VERSION, paths};
use glisa_i18n::tr;
use image::{ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};

use crate::fsutil::{read_untrusted, sha256_hex, write_private};

pub const MAX_FILE_BYTES: u64 = 20 << 20;
pub const MAX_SIDE: u32 = 8192;
pub const OUT_SIDE: u32 = 1024;
/// File extensions offered in the file dialog.
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];
const FORMATS: &[ImageFormat] = &[
    ImageFormat::Png,
    ImageFormat::Jpeg,
    ImageFormat::WebP,
    ImageFormat::Gif,
    ImageFormat::Bmp,
];

fn dir() -> PathBuf {
    paths::config_dir().join("pictures")
}

fn stem(device_id: &str) -> String {
    sha256_hex(device_id.as_bytes())[..32].to_string()
}

fn file_for(device_id: &str) -> PathBuf {
    dir().join(format!("{}.png", stem(device_id)))
}

fn layout_file(device_id: &str) -> PathBuf {
    dir().join(format!("{}.json", stem(device_id)))
}

/// Decodes untrusted image bytes into a clean PNG of at most `side` × `side`.
pub fn clean_png(data: &[u8], side: u32) -> Result<Vec<u8>, String> {
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| unsupported())?;
    match reader.format() {
        Some(f) if FORMATS.contains(&f) => {}
        _ => return Err(unsupported()),
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => tr!(
            "The picture is too big: at most {side} × {side} pixels",
            side = MAX_SIDE
        ),
        _ => unsupported(),
    })?;
    let img = if img.width() > side || img.height() > side {
        img.resize(side, side, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let mut out = Vec::new();
    img.to_rgba8()
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn unsupported() -> String {
    tr!("This file isn't a picture Glisa can use (PNG, JPEG, WebP, GIF or BMP)")
}

fn too_large() -> String {
    tr!(
        "The file is too large: pictures can be {mb} MB at most",
        mb = MAX_FILE_BYTES >> 20
    )
}

fn store(device_id: &str, data: &[u8]) -> Result<(), String> {
    let png = clean_png(data, OUT_SIDE)?;
    write_private(&file_for(device_id), &png)
        .map_err(|e| tr!("Can't save the picture ({error})", error = e))?;
    // Button positions placed on the previous picture (or on the official photo) don't fit
    // a new one: start over, keeping the "use on the device pages" choice.
    let layout = layout_get(device_id);
    if !layout.anchors.is_empty() {
        layout_set(
            device_id,
            Layout {
                use_on_pages: layout.use_on_pages,
                anchors: BTreeMap::new(),
            },
        )?;
    }
    Ok(())
}

/// Imports `src` as the picture of `device_id`.
pub fn import(src: &Path, device_id: &str) -> Result<(), String> {
    let data = read_untrusted(src, MAX_FILE_BYTES).map_err(|e| match e {
        None => too_large(),
        Some(e) => tr!("Can't read the file ({error})", error = e),
    })?;
    store(device_id, &data)
}

/// Downloads `url` (https) and imports it as the picture of `device_id`. Blocking.
pub fn import_url(url: &str, device_id: &str) -> Result<(), String> {
    let data = fetch(url.trim())?;
    store(device_id, &data)
}

/// Loopback, private, link-local, CGNAT, multicast, unspecified…: a picture link must
/// not reach into this computer or the local network.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// `https://host[:port]/…` → host, after checking the URL's shape.
fn https_host(url: &str) -> Option<(String, u16)> {
    if url.len() > 4096 || !url.bytes().all(|b| b.is_ascii_graphic()) {
        return None;
    }
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    if let Some(v6) = authority.strip_prefix('[') {
        let (host, tail) = v6.split_once(']')?;
        let port = tail
            .strip_prefix(':')
            .map(str::parse)
            .transpose()
            .ok()?
            .unwrap_or(443);
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((authority.to_string(), 443)),
    }
}

fn check_destination(url: &str) -> Result<(), String> {
    let bad = || tr!("Use a link to a picture that starts with https://");
    let (host, port) = https_host(url).ok_or_else(bad)?;
    let addrs: Vec<IpAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| tr!("Can't reach {host}", host = host))?
        .map(|a| a.ip())
        .collect();
    if addrs.is_empty() || !addrs.iter().all(|ip| is_public(*ip)) {
        return Err(tr!(
            "That link points to this computer or the local network"
        ));
    }
    Ok(())
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(format!("{APP_NAME}/{VERSION}"))
        .build()
        .into();
    let mut current = url.to_string();
    for _ in 0..4 {
        check_destination(&current)?;
        let mut resp = agent
            .get(&current)
            .call()
            .map_err(|e| tr!("Can't download the picture ({error})", error = e))?;
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            let next = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| tr!("Can't download the picture ({error})", error = status))?;
            current = if next.starts_with("https://") {
                next.to_string()
            } else if next.starts_with('/') {
                let (host, port) = https_host(&current)
                    .ok_or_else(|| tr!("Use a link to a picture that starts with https://"))?;
                if port == 443 {
                    format!("https://{host}{next}")
                } else {
                    format!("https://{host}:{port}{next}")
                }
            } else {
                return Err(tr!("Use a link to a picture that starts with https://"));
            };
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(tr!("Can't download the picture ({error})", error = status));
        }
        return resp
            .body_mut()
            .with_config()
            .limit(MAX_FILE_BYTES)
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(_) => too_large(),
                e => tr!("Can't download the picture ({error})", error = e),
            });
    }
    Err(tr!(
        "Can't download the picture ({error})",
        error = "too many redirects"
    ))
}

/// Where the user placed each button on their picture, and whether the device pages use
/// it. Positions are fractions (0..1) of the picture's width and height; keys are HID++
/// control ids in decimal.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub use_on_pages: bool,
    #[serde(default)]
    pub anchors: BTreeMap<String, Point>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

pub const MAX_ANCHORS: usize = 64;

impl Layout {
    /// Drops what doesn't make sense: unknown keys, NaN, out-of-range positions.
    pub fn sanitized(mut self) -> Self {
        self.anchors.retain(|k, p| {
            k.len() <= 5
                && k.parse::<u16>().is_ok()
                && !k.starts_with('+')
                && p.x.is_finite()
                && p.y.is_finite()
        });
        for p in self.anchors.values_mut() {
            p.x = p.x.clamp(0.0, 1.0);
            p.y = p.y.clamp(0.0, 1.0);
        }
        while self.anchors.len() > MAX_ANCHORS {
            let Some(k) = self.anchors.keys().next_back().cloned() else {
                break;
            };
            self.anchors.remove(&k);
        }
        self
    }
}

pub fn layout_get(device_id: &str) -> Layout {
    read_untrusted(&layout_file(device_id), 64 << 10)
        .ok()
        .and_then(|d| serde_json::from_slice::<Layout>(&d).ok())
        .map(Layout::sanitized)
        .unwrap_or_default()
}

pub fn layout_set(device_id: &str, layout: Layout) -> Result<Layout, String> {
    let layout = layout.sanitized();
    let data = serde_json::to_vec(&layout).map_err(|e| e.to_string())?;
    write_private(&layout_file(device_id), &data)
        .map_err(|e| tr!("Can't save the picture ({error})", error = e))?;
    Ok(layout)
}

/// The stored picture as a `data:` URL.
pub fn get(device_id: &str) -> Option<String> {
    let data = read_untrusted(&file_for(device_id), MAX_FILE_BYTES).ok()?;
    // Our own file, but still check it is a PNG before handing it to the page.
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(data)
    ))
}

/// Removes the picture and its layout.
pub fn remove(device_id: &str) -> std::io::Result<()> {
    for path in [file_for(device_id), layout_file(device_id)] {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn pictures_are_reencoded_and_reduced() {
        let out = clean_png(&png(2400, 1200), OUT_SIDE).unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!((img.width(), img.height()), (1024, 512));
        let small = clean_png(&png(40, 30), OUT_SIDE).unwrap();
        assert_eq!(image::load_from_memory(&small).unwrap().width(), 40);
    }

    #[test]
    fn junk_and_other_formats_are_refused() {
        assert!(clean_png(b"not an image at all", OUT_SIDE).is_err());
        assert!(clean_png(b"<svg xmlns='http://www.w3.org/2000/svg'/>", OUT_SIDE).is_err());
        // A PNG header that lies about its size.
        let mut bad = png(4, 4);
        bad[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(clean_png(&bad, OUT_SIDE).is_err());
    }

    #[test]
    fn links_must_be_https_and_public() {
        assert_eq!(
            https_host("https://example.com/a.png"),
            Some(("example.com".into(), 443))
        );
        assert_eq!(
            https_host("https://example.com:8443/a.png"),
            Some(("example.com".into(), 8443))
        );
        assert_eq!(
            https_host("https://[2001:db8::1]/a.png"),
            Some(("2001:db8::1".into(), 443))
        );
        assert_eq!(https_host("http://example.com/a.png"), None);
        assert_eq!(https_host("https://user@example.com/a.png"), None);
        assert_eq!(https_host("https://exa mple.com/a.png"), None);
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.2.20",
            "172.16.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "151.101.1.1", "2a00:1450:4003::1"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
        assert!(check_destination("https://127.0.0.1/x.png").is_err());
        assert!(check_destination("https://localhost/x.png").is_err());
    }

    #[test]
    fn layouts_are_sanitized() {
        let mut l = Layout {
            use_on_pages: true,
            anchors: BTreeMap::new(),
        };
        l.anchors.insert("82".into(), Point { x: 0.5, y: 1.7 });
        l.anchors.insert("x".into(), Point { x: 0.1, y: 0.1 });
        l.anchors.insert(
            "83".into(),
            Point {
                x: f64::NAN,
                y: 0.1,
            },
        );
        l.anchors.insert("99999".into(), Point { x: 0.1, y: 0.1 });
        let s = l.sanitized();
        assert_eq!(s.anchors.len(), 1);
        assert_eq!(s.anchors["82"], Point { x: 0.5, y: 1.0 });
        let many = Layout {
            use_on_pages: false,
            anchors: (0..100)
                .map(|i| (i.to_string(), Point { x: 0.0, y: 0.0 }))
                .collect(),
        };
        assert_eq!(many.sanitized().anchors.len(), MAX_ANCHORS);
    }
}
