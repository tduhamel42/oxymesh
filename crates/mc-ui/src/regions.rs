//! Region presets from `config/regions.toml` (the source of truth), read at run time from the
//! copy embedded at build time: only `[id]` sections with `key = value` lines.

use alloc::string::String;
use alloc::vec::Vec;

const REGIONS: &str = include_str!("../../../config/regions.toml");
const EU868: &str = include_str!("../../../config/eu868.toml");

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

/// The regulatory sub-band a frequency falls in (`config/eu868.toml`): its name and its duty
/// cycle in hundredths of a percent (1000 = 10 %).
pub fn duty_rule(freq_khz: u32) -> Option<(String, u32)> {
    let hz = u64::from(freq_khz) * 1000;
    let mut name = String::new();
    let (mut low, mut high, mut duty) = (0u64, 0u64, 0u32);
    let mut found = None;
    let mut close = |name: &str, low: u64, high: u64, duty: u32| {
        if found.is_none() && low <= hz && hz <= high && high > 0 {
            found = Some((String::from(name), duty));
        }
    };
    for line in EU868.lines().map(str::trim) {
        if line == "[[rule]]" {
            close(&name, low, high, duty);
            (low, high, duty) = (0, 0, 0);
            name.clear();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "name" => name = String::from(v),
            "low_hz" => low = v.parse().unwrap_or(0),
            "high_hz" => high = v.parse().unwrap_or(0),
            "duty_percent" => duty = centi(v),
            _ => {}
        }
    }
    close(&name, low, high, duty);
    found
}

/// "10.0" → 1000, "0.1" → 10.
fn centi(v: &str) -> u32 {
    let (i, f) = v.split_once('.').unwrap_or((v, ""));
    let f2: String = f.chars().chain(core::iter::repeat('0')).take(2).collect();
    i.parse::<u32>().unwrap_or(0) * 100 + f2.parse::<u32>().unwrap_or(0)
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

    #[test]
    fn duty_rule_of_the_french_frequency() {
        assert_eq!(duty_rule(869_618), Some((String::from("g3"), 1000)));
        assert_eq!(duty_rule(868_100), Some((String::from("g1"), 100)));
        assert_eq!(duty_rule(915_000), None);
    }
}
