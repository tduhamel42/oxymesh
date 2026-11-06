//! Region presets from `config/regions.toml` (the source of truth), read at run time from the
//! copy embedded at build time: only `[id]` sections with `key = value` lines.

use alloc::string::String;
use alloc::vec::Vec;

const REGIONS: &str = include_str!("../../../config/regions.toml");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub id: String,
    pub name: String,
    pub freq_khz: u32,
    pub bw_hz: u32,
    pub sf: u8,
    pub cr: u8,
}

pub fn presets() -> Vec<Region> {
    parse(REGIONS)
}

fn parse(src: &str) -> Vec<Region> {
    let mut out: Vec<Region> = Vec::new();
    let mut cur: Option<Region> = None;
    for line in src.lines().map(str::trim) {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(id) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.extend(cur.take());
            cur = Some(Region {
                id: String::from(id),
                name: String::from(id),
                freq_khz: 0,
                bw_hz: 0,
                sf: 0,
                cr: 0,
            });
            continue;
        }
        let (Some(r), Some((k, v))) = (cur.as_mut(), line.split_once('=')) else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "name" => r.name = String::from(v),
            "freq_khz" => r.freq_khz = v.parse().unwrap_or(0),
            "bw_hz" => r.bw_hz = v.parse().unwrap_or(0),
            "sf" => r.sf = v.parse().unwrap_or(0),
            "cr" => r.cr = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    out.extend(cur);
    out.retain(|r| r.freq_khz != 0 && r.bw_hz != 0 && r.sf != 0 && r.cr != 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn france_preset_is_read_from_the_config() {
        let p = presets();
        let fr = p.iter().find(|r| r.id == "fr").unwrap();
        assert_eq!(
            (fr.name.as_str(), fr.freq_khz, fr.bw_hz, fr.sf, fr.cr),
            ("France", 869_618, 62_500, 8, 8)
        );
    }
}
