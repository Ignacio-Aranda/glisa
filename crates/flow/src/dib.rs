//! Device-independent bitmaps (Windows `CF_DIB` / `CF_DIBV5`) ↔ the formats that travel.
//!
//! Images travel as PNG. When the Windows clipboard only has a bitmap (Paint, the Snipping
//! Tool and most programs put `CF_DIB`/`CF_DIBV5`; only some add the registered "PNG"
//! format), it is converted to PNG here, on the computer that copied it. That bitmap is this
//! computer's own clipboard content; the header is checked (size, depth, dimensions within
//! the image caps) before anything is allocated. Received images are never decoded: a BMP
//! from a Python peer becomes `CF_DIB` by dropping its 14-byte file header.

use crate::imageinfo::acceptable;

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn i32_at(b: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn channel(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    let x = (v & mask) >> shift;
    if bits >= 8 {
        (x >> (bits - 8)) as u8
    } else {
        ((x * 255) / ((1 << bits) - 1)) as u8
    }
}

/// A 24- or 32-bit DIB (BITMAPINFOHEADER, V4 or V5; BI_RGB or BI_BITFIELDS) as PNG.
pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let hsize = u32_at(dib, 0)? as usize;
    if !(40..=124).contains(&hsize) {
        return None;
    }
    let width = i32_at(dib, 4)?;
    let height = i32_at(dib, 8)?;
    let bpp = u16_at(dib, 14)?;
    let compression = u32_at(dib, 16)?;
    let clr_used = u32_at(dib, 32)? as usize;
    if width <= 0 || height == 0 || height == i32::MIN || !(bpp == 24 || bpp == 32) {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    if !acceptable(w as u64, h as u64) {
        return None;
    }
    let (masks, mut offset) = match compression {
        0 => ([0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0], hsize),
        3 if hsize >= 52 => {
            let a = if hsize >= 56 { u32_at(dib, 52)? } else { 0 };
            (
                [u32_at(dib, 40)?, u32_at(dib, 44)?, u32_at(dib, 48)?, a],
                hsize,
            )
        }
        3 => (
            [
                u32_at(dib, hsize)?,
                u32_at(dib, hsize + 4)?,
                u32_at(dib, hsize + 8)?,
                0,
            ],
            hsize + 12,
        ),
        _ => return None,
    };
    offset += clr_used.min(256) * 4;
    let stride = (w * bpp as usize).div_ceil(32) * 4;
    if dib.len() < offset + stride * h {
        return None;
    }
    let topdown = height < 0;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = if topdown { y } else { h - 1 - y };
        let line = &dib[offset + row * stride..offset + (row + 1) * stride];
        for x in 0..w {
            if bpp == 24 {
                let p = &line[x * 3..x * 3 + 3];
                rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
            } else {
                let v = u32::from_le_bytes(line[x * 4..x * 4 + 4].try_into().ok()?);
                let a = if masks[3] != 0 {
                    channel(v, masks[3])
                } else {
                    255
                };
                rgba.extend_from_slice(&[
                    channel(v, masks[0]),
                    channel(v, masks[1]),
                    channel(v, masks[2]),
                    a,
                ]);
            }
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().ok()?;
        wr.write_image_data(&rgba).ok()?;
    }
    Some(out)
}

/// The DIB inside a BMP file (drops the 14-byte BITMAPFILEHEADER).
pub fn bmp_to_dib(bmp: &[u8]) -> Option<&[u8]> {
    (bmp.len() > 14 && bmp.starts_with(b"BM")).then(|| &bmp[14..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: i32, h: i32, bpp: u16, compression: u32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&w.to_le_bytes());
        d.extend_from_slice(&h.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&bpp.to_le_bytes());
        d.extend_from_slice(&compression.to_le_bytes());
        d.extend_from_slice(&[0; 20]);
        d
    }

    fn decode(png_data: &[u8]) -> (u32, u32, Vec<u8>) {
        let dec = png::Decoder::new(std::io::Cursor::new(png_data));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    #[test]
    fn bottom_up_24_bit() {
        // 2×2: bottom row first; rows padded to 4 bytes
        let mut d = header(2, 2, 24, 0);
        d.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // bottom: red, green
        d.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]); // top: blue, white
        let (w, h, px) = decode(&dib_to_png(&d).unwrap());
        assert_eq!((w, h), (2, 2));
        assert_eq!(
            px,
            vec![
                0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255
            ]
        );
    }

    #[test]
    fn top_down_32_bit_bitfields() {
        let mut d = header(1, -1, 32, 3);
        d.extend_from_slice(&0x00FF_0000u32.to_le_bytes());
        d.extend_from_slice(&0x0000_FF00u32.to_le_bytes());
        d.extend_from_slice(&0x0000_00FFu32.to_le_bytes());
        d.extend_from_slice(&0x0011_2233u32.to_le_bytes());
        let (_, _, px) = decode(&dib_to_png(&d).unwrap());
        assert_eq!(px, vec![0x11, 0x22, 0x33, 255]);
    }

    #[test]
    fn refuses_what_it_doesnt_know() {
        assert!(dib_to_png(&header(2, 2, 8, 0)).is_none()); // palette
        assert!(dib_to_png(&header(20000, 2, 24, 0)).is_none()); // too wide
        assert!(dib_to_png(&header(4, 4, 24, 0)).is_none()); // truncated
        assert!(dib_to_png(b"short").is_none());
        assert_eq!(bmp_to_dib(b"BMxxxxxxxxxxxxyz"), Some(&b"yz"[..]));
    }
}
