//! The C++ firmware's settings file (`/prefs.json`, `src/helpers/ConfigSerializer.cpp`, MeshCore,
//! MIT): JSON-like, unquoted keys, nested objects, quoted strings with `\"` `\\` `\n` `\r`
//! escapes, bare values (`[0-9a-z.-]`). Read without allocation; numbers stay text until the
//! caller scales them (no floats).

/// A value as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value<'a> {
    /// Between the quotes, escapes not undone.
    Str(&'a [u8]),
    Bare(&'a [u8]),
}

const DEPTH: usize = 4;

/// Calls `visit(path, key, value)` for every leaf: `path` is the enclosing keys. `false` if the
/// text is malformed (what came before it was visited).
pub fn walk<'a>(src: &'a [u8], mut visit: impl FnMut(&[&'a [u8]], &'a [u8], Value<'a>)) -> bool {
    let mut path: heapless::Vec<&[u8], DEPTH> = heapless::Vec::new();
    let mut i = 0usize;
    let ws = |i: &mut usize| {
        while src.get(*i).is_some_and(|c| c.is_ascii_whitespace()) {
            *i += 1;
        }
    };
    ws(&mut i);
    if src.get(i) != Some(&b'{') {
        return false;
    }
    i += 1;
    loop {
        ws(&mut i);
        match src.get(i) {
            Some(b'}') => {
                i += 1;
                if path.pop().is_none() {
                    return true;
                }
                continue;
            }
            Some(b',') => {
                i += 1;
                continue;
            }
            Some(_) => {}
            None => return false,
        }
        // Key.
        let k0 = i;
        while src
            .get(i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            i += 1;
        }
        let Some(key) = src.get(k0..i).filter(|k| !k.is_empty()) else {
            return false;
        };
        ws(&mut i);
        if src.get(i) != Some(&b':') {
            return false;
        }
        i += 1;
        ws(&mut i);
        match src.get(i) {
            Some(b'{') => {
                i += 1;
                if path.push(key).is_err() {
                    return false;
                }
            }
            Some(b'"') => {
                i += 1;
                let v0 = i;
                loop {
                    match src.get(i) {
                        Some(b'\\') => i += 2,
                        Some(b'"') => break,
                        Some(_) => i += 1,
                        None => return false,
                    }
                }
                let Some(v) = src.get(v0..i) else {
                    return false;
                };
                i += 1;
                visit(&path, key, Value::Str(v));
            }
            Some(_) => {
                let v0 = i;
                while src.get(i).is_some_and(|c| {
                    c.is_ascii_digit() || c.is_ascii_lowercase() || *c == b'.' || *c == b'-'
                }) {
                    i += 1;
                }
                let Some(v) = src.get(v0..i).filter(|v| !v.is_empty()) else {
                    return false;
                };
                visit(&path, key, Value::Bare(v));
            }
            None => return false,
        }
    }
}

/// Undoes the escapes into `out`; the length written (cut to `out`).
pub fn unescape(raw: &[u8], out: &mut [u8]) -> usize {
    let mut n = 0;
    let mut it = raw.iter();
    while let Some(&c) = it.next() {
        let b = if c == b'\\' {
            match it.next() {
                Some(b'n') => b'\n',
                Some(b'r') => b'\r',
                Some(&e) => e,
                None => break,
            }
        } else {
            c
        };
        match out.get_mut(n) {
            Some(o) => *o = b,
            None => break,
        }
        n += 1;
    }
    n
}

/// A decimal (`-12.5`, `869.6180`, `true`) times 10^`digits`, truncated; `true` is 1.
pub fn fixed(v: &[u8], digits: u32) -> Option<i64> {
    if v == b"true" {
        return Some(10i64.pow(digits));
    }
    if v == b"false" {
        return Some(0);
    }
    let (neg, v) = match v.strip_prefix(b"-") {
        Some(r) => (true, r),
        None => (false, v),
    };
    let (int, frac) = match v.iter().position(|&c| c == b'.') {
        Some(p) => (v.get(..p)?, v.get(p + 1..)?),
        None => (v, &b""[..]),
    };
    if int.len() > 12
        || !int.iter().chain(frac).all(u8::is_ascii_digit)
        || (int.is_empty() && frac.is_empty())
    {
        return None;
    }
    let mut x: i64 = int.iter().fold(0, |a, &c| a * 10 + i64::from(c - b'0'));
    for k in 0..digits as usize {
        x = x * 10 + frac.get(k).map_or(0, |&c| i64::from(c - b'0'));
    }
    Some(if neg { -x } else { x })
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    #[test]
    fn walks_nested_objects() {
        let src = br#"{name:"Tan \"M5\"",lat:45.123456,radio:{freq:869.6180,sf:8},custom:{},comp:{defs_key:"00FF"}}"#;
        let mut seen: Vec<(Vec<u8>, Vec<u8>, Value<'_>)> = Vec::new();
        assert!(walk(src, |p, k, v| {
            seen.push((p.concat(), k.to_vec(), v));
        }));
        assert_eq!(seen.len(), 5);
        assert_eq!(seen[0].2, Value::Str(br#"Tan \"M5\""#));
        assert_eq!(
            (seen[2].0.as_slice(), seen[2].1.as_slice()),
            (&b"radio"[..], &b"freq"[..])
        );
        assert_eq!(seen[4].0, b"comp");
        let mut out = [0u8; 16];
        let n = unescape(br#"Tan \"M5\""#, &mut out);
        assert_eq!(&out[..n], br#"Tan "M5""#);
    }

    #[test]
    fn numbers_without_floats() {
        assert_eq!(fixed(b"869.6180", 3), Some(869_618));
        assert_eq!(fixed(b"62.5000", 3), Some(62_500));
        assert_eq!(fixed(b"-5.654321", 6), Some(-5_654_321));
        assert_eq!(fixed(b"0", 3), Some(0));
        assert_eq!(fixed(b"22", 0), Some(22));
        assert_eq!(fixed(b"1.9999", 0), Some(1));
        assert_eq!(fixed(b"abc", 0), None);
        assert_eq!(fixed(b"-", 0), None);
    }

    #[test]
    fn malformed() {
        assert!(!walk(b"", |_, _, _| {}));
        assert!(!walk(b"{name:\"x", |_, _, _| {}));
        assert!(
            !walk(b"{a:{b:{c:{d:{e:{f:1}}}}}}", |_, _, _| {}),
            "too deep"
        );
        assert!(!walk(b"{:1}", |_, _, _| {}));
    }
}
