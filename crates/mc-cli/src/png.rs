//! A minimal PNG writer for the panel frames (8-bit grayscale, stored deflate): no dependency
//! for a few hundred KB of output.

use std::io::Write;

fn crc32(data: &[&[u8]]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for part in data {
        for &b in *part {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                };
            }
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// `pixels`: `w × h` gray levels, row by row.
pub fn encode(w: u32, h: u32, pixels: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w as usize + 1) * h as usize);
    for row in pixels.chunks(w as usize) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    // zlib: header, stored blocks of at most 65,535 bytes, Adler-32.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let n = b.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8 bits, grayscale
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The panel frame, each pixel scaled to `scale × scale`, ink black on white.
pub fn write_frame(
    path: &std::path::Path,
    frame: &mc_ui::panel::Frame,
    scale: u32,
) -> std::io::Result<()> {
    let (w, h) = (mc_ui::panel::WIDTH * scale, mc_ui::panel::HEIGHT * scale);
    let mut px = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            px.push(if frame.ink(x / scale, y / scale) {
                0
            } else {
                255
            });
        }
    }
    std::fs::File::create(path)?.write_all(&encode(w, h, &px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_iend() {
        // The IEND chunk's CRC is a well-known constant.
        assert_eq!(crc32(&[b"IEND", &[]]), 0xAE42_6082);
    }
}
