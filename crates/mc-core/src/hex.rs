//! Allocation-free hexadecimal display.

use core::fmt;

/// Displays bytes as lowercase hexadecimal, without separators.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Hex<'a>(pub &'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Decodes a hexadecimal string into `out`. Returns the number of bytes written.
///
/// Whitespace and underscores are ignored, so test vectors can be spaced out for readability.
pub fn decode_into(s: &str, out: &mut [u8]) -> Option<usize> {
    let mut n = 0usize;
    let mut high: Option<u8> = None;
    for c in s.bytes() {
        if c.is_ascii_whitespace() || c == b'_' {
            continue;
        }
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        };
        match high.take() {
            None => high = Some(v),
            Some(h) => {
                *out.get_mut(n)? = (h << 4) | v;
                n += 1;
            }
        }
    }
    if high.is_some() { None } else { Some(n) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let mut buf = [0u8; 4];
        assert_eq!(decode_into("de ad_BE ef", &mut buf), Some(4));
        assert_eq!(buf, [0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(decode_into("abc", &mut buf), None);
        assert_eq!(decode_into("0g", &mut buf), None);
        assert_eq!(decode_into("0102030405", &mut buf), None);
    }
}
