//! Image type and size from the first bytes of the file — nothing is decoded.
//!
//! A copied image that arrives from the other computer is untrusted: before it goes on this
//! computer's clipboard (where any application may decode it) its signature must match the
//! type the sender declared and its dimensions must be sensible. Both come from the file
//! header, parsed here without decoding a single pixel.
//!
//! ```text
//!     sniff(head) -> "image/png" | "image/jpeg" | "image/webp" | "image/bmp" | None
//!     inspect(path) -> (mime, width, height); Err if it isn't a valid header
//! ```

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Pixels per side.
pub const MAX_SIDE: u64 = 16384;
/// 64 megapixels (what Qt's default allocation limit can decode).
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

const SOF: [u8; 13] = [
    0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF,
];

pub fn sniff(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if head.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else if head.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

type R = Result<(u64, u64), String>;

fn read_at<F: Read + Seek>(f: &mut F, offset: u64, n: usize) -> Result<Vec<u8>, String> {
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; n];
    let mut got = 0;
    while got < n {
        match f.read(&mut buf[got..]) {
            Ok(0) => return Err("truncated image header".into()),
            Ok(k) => got += k,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(buf)
}

fn be32(b: &[u8]) -> u64 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64
}

fn png<F: Read + Seek>(f: &mut F) -> R {
    let head = read_at(f, 8, 16)?;
    if &head[4..8] != b"IHDR" || be32(&head[0..4]) != 13 {
        return Err("PNG without IHDR".into());
    }
    Ok((be32(&head[8..12]), be32(&head[12..16])))
}

fn jpeg<F: Read + Seek>(f: &mut F) -> R {
    let mut pos = 2u64;
    for _ in 0..4096 {
        let marker = read_at(f, pos, 2)?;
        if marker[0] != 0xFF {
            return Err("bad JPEG marker".into());
        }
        let code = marker[1];
        if code == 0xFF {
            pos += 1; // fill byte
            continue;
        }
        if code == 0xD8 || code == 0x01 || (0xD0..=0xD7).contains(&code) {
            pos += 2;
            continue;
        }
        if code == 0xD9 || code == 0xDA {
            break; // end of image / start of scan before any frame header
        }
        let l = read_at(f, pos + 2, 2)?;
        let length = u16::from_be_bytes([l[0], l[1]]) as u64;
        if length < 2 {
            return Err("bad JPEG segment".into());
        }
        if SOF.contains(&code) {
            if length < 7 {
                return Err("bad JPEG frame header".into());
            }
            let d = read_at(f, pos + 4, 5)?;
            let h = u16::from_be_bytes([d[1], d[2]]) as u64;
            let w = u16::from_be_bytes([d[3], d[4]]) as u64;
            return Ok((w, h));
        }
        pos += 2 + length;
    }
    Err("JPEG without a frame header".into())
}

fn webp<F: Read + Seek>(f: &mut F) -> R {
    let kind = read_at(f, 12, 4)?;
    match &kind[..] {
        b"VP8 " => {
            let tag = read_at(f, 20, 10)?;
            if &tag[3..6] != b"\x9d\x01\x2a" {
                return Err("bad VP8 frame".into());
            }
            let w = u16::from_le_bytes([tag[6], tag[7]]) as u64;
            let h = u16::from_le_bytes([tag[8], tag[9]]) as u64;
            Ok((w & 0x3FFF, h & 0x3FFF))
        }
        b"VP8L" => {
            let d = read_at(f, 20, 5)?;
            if d[0] != 0x2F {
                return Err("bad VP8L signature".into());
            }
            let bits = u32::from_le_bytes([d[1], d[2], d[3], d[4]]) as u64;
            Ok(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => {
            let d = read_at(f, 24, 6)?;
            let w = u32::from_le_bytes([d[0], d[1], d[2], 0]) as u64 + 1;
            let h = u32::from_le_bytes([d[3], d[4], d[5], 0]) as u64 + 1;
            Ok((w, h))
        }
        _ => Err("unknown WebP chunk".into()),
    }
}

fn bmp<F: Read + Seek>(f: &mut F) -> R {
    let s = read_at(f, 14, 4)?;
    let size = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
    if size == 12 {
        let d = read_at(f, 18, 4)?;
        return Ok((
            u16::from_le_bytes([d[0], d[1]]) as u64,
            u16::from_le_bytes([d[2], d[3]]) as u64,
        ));
    }
    if (40..=124).contains(&size) {
        let d = read_at(f, 18, 8)?;
        let w = i32::from_le_bytes([d[0], d[1], d[2], d[3]]);
        let h = i32::from_le_bytes([d[4], d[5], d[6], d[7]]);
        if w <= 0 || h == i32::MIN {
            return Err("bad BMP size".into());
        }
        return Ok((w as u64, h.unsigned_abs() as u64));
    }
    Err("unknown BMP header".into())
}

/// (mime, width, height) from the header of `f`.
pub fn inspect_reader<F: Read + Seek>(f: &mut F) -> Result<(&'static str, u64, u64), String> {
    let mut head = [0u8; 16];
    let mut got = 0;
    while got < head.len() {
        match f.read(&mut head[got..]) {
            Ok(0) => break,
            Ok(k) => got += k,
            Err(e) => return Err(e.to_string()),
        }
    }
    let mime = sniff(&head[..got]).ok_or("not a PNG, JPEG, WebP or BMP image")?;
    let (w, h) = match mime {
        "image/png" => png(f)?,
        "image/jpeg" => jpeg(f)?,
        "image/webp" => webp(f)?,
        _ => bmp(f)?,
    };
    if w == 0 || h == 0 {
        return Err("image without pixels".into());
    }
    Ok((mime, w, h))
}

/// (mime, width, height) from the header of the file at `path`.
pub fn inspect(path: &Path) -> Result<(&'static str, u64, u64), String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    inspect_reader(&mut f)
}

pub fn acceptable(width: u64, height: u64) -> bool {
    0 < width
        && width <= MAX_SIDE
        && 0 < height
        && height <= MAX_SIDE
        && width * height <= MAX_PIXELS
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc ^= *b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
        out
    }

    /// A PNG whose header claims w × h (a stored-zlib IDAT: valid, never decoded here).
    pub fn png_bytes(w: u32, h: u32, tail: &[u8]) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        out.extend(chunk(b"IHDR", &ihdr));
        out.extend(chunk(
            b"IDAT",
            &[0x78, 0x01, 0x01, 0x00, 0x00, 0xff, 0xff, 0, 0, 0, 1],
        ));
        out.extend_from_slice(tail);
        out.extend(chunk(b"IEND", b""));
        out
    }

    pub fn jpeg_bytes(w: u16, h: u16) -> Vec<u8> {
        let mut out = b"\xff\xd8\xff\xe0".to_vec();
        out.extend_from_slice(&16u16.to_be_bytes());
        out.extend_from_slice(b"JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00");
        out.extend_from_slice(b"\xff\xc0");
        out.extend_from_slice(&17u16.to_be_bytes());
        out.push(8);
        out.extend_from_slice(&h.to_be_bytes());
        out.extend_from_slice(&w.to_be_bytes());
        out.push(3);
        out.extend_from_slice(b"\x01\x22\x00\x02\x11\x01\x03\x11\x01\xff\xd9");
        out
    }

    fn check(data: &[u8]) -> Result<(&'static str, u64, u64), String> {
        inspect_reader(&mut Cursor::new(data.to_vec()))
    }

    #[test]
    fn headers() {
        let vp8l = ((640u32 - 1) | ((480 - 1) << 14)).to_le_bytes();
        let mut webp_l = b"RIFF\x00\x00\x00\x00WEBPVP8L\x05\x00\x00\x00\x2f".to_vec();
        webp_l.extend_from_slice(&vp8l);
        let mut vp8x = vec![0u8; 4];
        vp8x.extend_from_slice(&1919u32.to_le_bytes()[..3]);
        vp8x.extend_from_slice(&1079u32.to_le_bytes()[..3]);
        let mut webp_x = b"RIFF\x00\x00\x00\x00WEBPVP8X\x0a\x00\x00\x00".to_vec();
        webp_x.extend_from_slice(&vp8x);
        let mut webp_8 =
            b"RIFF\x00\x00\x00\x00WEBPVP8 \x0a\x00\x00\x00\x00\x00\x00\x9d\x01\x2a".to_vec();
        webp_8.extend_from_slice(&300u16.to_le_bytes());
        webp_8.extend_from_slice(&200u16.to_le_bytes());
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&64i32.to_le_bytes());
        bmp.extend_from_slice(&(-32i32).to_le_bytes());
        bmp.extend_from_slice(&[0; 20]);
        assert_eq!(
            check(&png_bytes(479, 209, b"")),
            Ok(("image/png", 479, 209))
        );
        assert_eq!(check(&jpeg_bytes(1024, 768)), Ok(("image/jpeg", 1024, 768)));
        assert_eq!(check(&webp_l), Ok(("image/webp", 640, 480)));
        assert_eq!(check(&webp_x), Ok(("image/webp", 1920, 1080)));
        assert_eq!(check(&webp_8), Ok(("image/webp", 300, 200)));
        assert_eq!(check(&bmp), Ok(("image/bmp", 64, 32)));
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0; 20]);
        assert!(check(&gif).is_err());
        assert!(check(b"\x89PNG\r\n\x1a\n\x00\x00\x00\x00").is_err());
        assert!(check(b"\xff\xd8\xff\xd9").is_err());
        let mut idat_first = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIDAT".to_vec();
        idat_first.extend_from_slice(&[0; 8]);
        assert!(check(&idat_first).is_err());
        assert!(acceptable(3840, 2160));
        assert!(!acceptable(20000, 100));
        assert!(!acceptable(16000, 16000));
        assert_eq!(sniff(&png_bytes(3, 2, b"")), Some("image/png"));
    }
}
