//! Regulatory rules and the duty-cycle budget kept by the node (taken from loranet, loranet's
//! ADR-0002 and ADR-0006; values in config/eu868.toml).
//!
//! Values are never hard-coded here: they come from `config/eu868.toml`.

use crate::{Duration, Instant, RadioConfig};

/// One permitted transmission regime: frequency range, maximum power, duty cycle.
///
/// A sub-band with several regimes (g4: 5 mW unlimited or 25 mW at 1 %) yields several rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub low_hz: u32,
    pub high_hz: u32,
    /// Maximum effective radiated power, in hundredths of a dBm.
    pub max_erp_cdbm: i32,
    /// Fraction of the window allowed for transmitting, in millionths (1 % = 10 000).
    pub duty_ppm: u32,
    /// Sliding window (1 h in the EU).
    pub window: Duration,
}

impl Rule {
    /// Does the transmission fit entirely within the range, at a permitted power?
    pub const fn admits(&self, cfg: &RadioConfig, erp_cdbm: i32) -> bool {
        let (lo, hi) = cfg.span_hz();
        lo >= self.low_hz && hi <= self.high_hz && erp_cdbm <= self.max_erp_cdbm
    }

    /// Transmit time allowed per window, in nanoseconds.
    pub const fn budget_ns(&self) -> u64 {
        ((self.window.as_nanos() as u128 * self.duty_ppm as u128) / 1_000_000) as u64
    }
}

/// Rule that applies to a transmission: the most permissive among those that admit it.
pub fn applicable_rule(rules: &[Rule], cfg: &RadioConfig, erp_cdbm: i32) -> Option<usize> {
    let mut best: Option<(usize, u32)> = None;
    for (i, r) in rules.iter().enumerate() {
        if r.admits(cfg, erp_cdbm) && best.is_none_or(|(_, d)| r.duty_ppm > d) {
            best = Some((i, r.duty_ppm));
        }
    }
    best.map(|(i, _)| i)
}

/// The EU 863-870 MHz profile (`config/eu868.toml`), read by [`parse`].
pub const EU868: &str = include_str!("../../../config/eu868.toml");

/// Most rules a profile may hold.
pub const RULES_MAX: usize = 12;

/// The rules of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    rules: [Rule; RULES_MAX],
    len: usize,
}

impl Rules {
    pub fn as_slice(&self) -> &[Rule] {
        self.rules.get(..self.len).unwrap_or(&[])
    }

    /// Is any part of the band `[low, high]` covered by the profile? Outside it (another
    /// region), the profile says nothing.
    pub fn covers(&self, cfg: &RadioConfig) -> bool {
        let (lo, hi) = cfg.span_hz();
        self.as_slice()
            .iter()
            .any(|r| lo <= r.high_hz && hi >= r.low_hz)
    }
}

/// Reads a profile in the TOML layout of `config/eu868.toml`: a top-level `window_s`, then
/// `[[rule]]` tables with `low_hz`, `high_hz`, `max_erp_mw` and `duty_percent`; other keys
/// (names, sources, flags) are skipped. `None` if a rule is incomplete or a number unreadable.
pub fn parse(src: &str) -> Option<Rules> {
    #[derive(Default, Clone, Copy)]
    struct Partial {
        low: Option<u32>,
        high: Option<u32>,
        erp_cdbm: Option<i32>,
        duty_ppm: Option<u32>,
    }
    let mut window_s: Option<u64> = None;
    let mut out = Rules {
        rules: [Rule {
            low_hz: 0,
            high_hz: 0,
            max_erp_cdbm: 0,
            duty_ppm: 0,
            window: Duration::from_secs(0),
        }; RULES_MAX],
        len: 0,
    };
    let mut cur: Option<Partial> = None;
    let mut done: [Option<Partial>; RULES_MAX] = [None; RULES_MAX];
    let mut n = 0;
    let mut finish = |p: Option<Partial>, n: &mut usize| -> Option<()> {
        if let Some(p) = p {
            *done.get_mut(*n)? = Some(p);
            *n += 1;
        }
        Some(())
    };
    for line in src.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[rule]]" {
            finish(cur.take(), &mut n)?;
            cur = Some(Partial::default());
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match (cur.as_mut(), k) {
            (None, "window_s") => window_s = Some(v.parse().ok()?),
            (Some(r), "low_hz") => r.low = Some(v.parse().ok()?),
            (Some(r), "high_hz") => r.high = Some(v.parse().ok()?),
            (Some(r), "max_erp_mw") => r.erp_cdbm = Some(milli_mw_to_cdbm(decimal_milli(v)?)?),
            (Some(r), "duty_percent") => {
                r.duty_ppm = Some(u32::try_from(decimal_milli(v)? * 10).ok()?);
            }
            _ => {}
        }
    }
    finish(cur.take(), &mut n)?;
    let window = Duration::from_secs(window_s?);
    for (slot, p) in out.rules.iter_mut().zip(done.iter().take(n)) {
        let p = (*p)?;
        *slot = Rule {
            low_hz: p.low?,
            high_hz: p.high?,
            max_erp_cdbm: p.erp_cdbm?,
            duty_ppm: p.duty_ppm?,
            window,
        };
    }
    out.len = n;
    Some(out)
}

/// A non-negative decimal number (`25`, `0.1`), in thousandths.
fn decimal_milli(v: &str) -> Option<u64> {
    let (int, frac) = v.split_once('.').unwrap_or((v, ""));
    let mut x: u64 = int.parse().ok()?;
    let mut digits = 0;
    for c in frac.bytes() {
        if !c.is_ascii_digit() {
            return None;
        }
        if digits < 3 {
            x = x.checked_mul(10)? + u64::from(c - b'0');
            digits += 1;
        }
    }
    for _ in digits..3 {
        x = x.checked_mul(10)?;
    }
    Some(x)
}

/// Power in thousandths of a milliwatt to hundredths of a dBm, rounded: 10·log10(mW), in
/// integers (a fixed-point base-2 logarithm, then × log10(2)).
pub fn milli_mw_to_cdbm(milli_mw: u64) -> Option<i32> {
    if milli_mw == 0 {
        return None;
    }
    // log2(x) in Q32: the integer part from the bit length, then 32 fraction bits by squaring.
    let ip = 63 - u64::from(milli_mw.leading_zeros());
    let mut y: u128 = (u128::from(milli_mw) << 32) >> ip; // in [1, 2) as Q32
    let mut frac: u64 = 0;
    for i in 1..=32 {
        y = (y * y) >> 32;
        if y >= 2u128 << 32 {
            y >>= 1;
            frac |= 1 << (32 - i);
        }
    }
    let log2_q32 = (u128::from(ip) << 32) | u128::from(frac);
    // log10(2) in Q32.
    const LOG10_2_Q32: u128 = 1_292_913_986;
    // 1000 × log10(x) (x in thousandths of a mW), rounded; minus 3000 for the thousandths.
    let v = (log2_q32 * LOG10_2_Q32 * 1000 + (1u128 << 63)) >> 64;
    i32::try_from(v).ok().map(|v| v - 3000)
}

/// Number of buckets in the budget window.
pub const BUCKETS: usize = 62;

/// Bucketed duty-cycle budget kept by the node, **conservative**: it never underestimates usage
/// over an exact sliding window.
///
/// The window `W` is split into buckets of `W / (BUCKETS − 2)`. A transmission is counted in full
/// in the bucket where it starts; the last `BUCKETS` buckets always cover at least `W` plus the
/// duration of one transmission, provided no transmission is longer than a bucket (checked by
/// [`DutyBudget::can_transmit`]).
///
/// Instants passed to the methods must be **non-decreasing**: querying a future instant advances
/// the buckets, and a later query at an earlier instant would see an emptied budget. To probe the
/// future, work on a copy (`clone`).
#[derive(Debug, Clone)]
pub struct DutyBudget {
    bucket_ns: u64,
    limit_us: u64,
    used_us: [u32; BUCKETS],
    current: u64,
}

impl DutyBudget {
    /// `margin_ppm` subtracts a safety margin from the budget (10 000 = 1 %), to cover clock
    /// drift and rounding.
    pub fn new(rule: &Rule, margin_ppm: u32) -> Self {
        let bucket_ns = (rule.window.as_nanos() / (BUCKETS as u64 - 2)).max(1);
        let keep = 1_000_000u128.saturating_sub(u128::from(margin_ppm));
        let limit_ns = u128::from(rule.budget_ns()) * keep / 1_000_000;
        Self {
            bucket_ns,
            limit_us: (limit_ns / 1_000) as u64,
            used_us: [0; BUCKETS],
            current: 0,
        }
    }

    fn slot(&mut self, index: u64) -> Option<&mut u32> {
        self.used_us.get_mut((index % BUCKETS as u64) as usize)
    }

    fn advance(&mut self, now: Instant) {
        let idx = now.as_nanos() / self.bucket_ns;
        if idx <= self.current {
            return;
        }
        if idx - self.current >= BUCKETS as u64 {
            self.used_us = [0; BUCKETS];
        } else {
            for i in self.current + 1..=idx {
                if let Some(s) = self.slot(i) {
                    *s = 0;
                }
            }
        }
        self.current = idx;
    }

    fn need_us(airtime: Duration) -> u64 {
        airtime.as_nanos().div_ceil(1_000)
    }

    /// Transmit time counted over the current window, in microseconds.
    pub fn used_us(&mut self, now: Instant) -> u64 {
        self.advance(now);
        self.used_us.iter().map(|&u| u64::from(u)).sum()
    }

    /// Total budget per window, margin deducted, in microseconds.
    pub const fn limit_us(&self) -> u64 {
        self.limit_us
    }

    /// Does a transmission of duration `airtime` starting at `now` fit in the budget?
    pub fn can_transmit(&mut self, now: Instant, airtime: Duration) -> bool {
        airtime.as_nanos() <= self.bucket_ns
            && self.used_us(now) + Self::need_us(airtime) <= self.limit_us
    }

    /// Records a transmission starting at `now`.
    pub fn record(&mut self, now: Instant, airtime: Duration) {
        self.advance(now);
        let need = u32::try_from(Self::need_us(airtime)).unwrap_or(u32::MAX);
        let cur = self.current;
        if let Some(s) = self.slot(cur) {
            *s = s.saturating_add(need);
        }
    }

    /// First instant `≥ now` at which a transmission of duration `airtime` will be allowed, or
    /// `None` if it never will be (longer than a bucket or than the budget).
    pub fn next_opportunity(&mut self, now: Instant, airtime: Duration) -> Option<Instant> {
        if airtime.as_nanos() > self.bucket_ns || Self::need_us(airtime) > self.limit_us {
            return None;
        }
        let mut used = self.used_us(now);
        let need = Self::need_us(airtime);
        if used + need <= self.limit_us {
            return Some(now);
        }
        // At the start of bucket `current + k`, bucket `current + k − BUCKETS` leaves the window.
        for k in 1..=BUCKETS as u64 {
            let leaving = self.current + k;
            let freed = self
                .used_us
                .get((leaving % BUCKETS as u64) as usize)
                .copied()
                .unwrap_or(0);
            used = used.saturating_sub(u64::from(freed));
            if used + need <= self.limit_us {
                return Some(Instant::from_nanos(leaving * self.bucket_ns));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::{Bandwidth, LoRa, Modulation};

    fn g1() -> Rule {
        Rule {
            low_hz: 868_000_000,
            high_hz: 868_600_000,
            max_erp_cdbm: 1398,
            duty_ppm: 10_000,
            window: Duration::from_secs(3600),
        }
    }

    fn cfg(freq_hz: u32) -> RadioConfig {
        RadioConfig {
            freq_hz,
            modulation: Modulation::LoRa(LoRa::new(7, Bandwidth::Bw125)),
        }
    }

    #[test]
    fn power_in_dbm() {
        assert_eq!(milli_mw_to_cdbm(1000), Some(0));
        assert_eq!(milli_mw_to_cdbm(25_000), Some(1398));
        assert_eq!(milli_mw_to_cdbm(500_000), Some(2699));
        assert_eq!(milli_mw_to_cdbm(5_000), Some(699));
        assert_eq!(milli_mw_to_cdbm(100), Some(-1000));
        assert_eq!(milli_mw_to_cdbm(0), None);
    }

    #[test]
    fn the_eu_profile_reads() {
        let r = parse(EU868).unwrap();
        let s = r.as_slice();
        assert_eq!(s.len(), 7);
        let g3 = s.iter().find(|r| r.low_hz == 869_400_000).copied().unwrap();
        assert_eq!(
            g3,
            Rule {
                low_hz: 869_400_000,
                high_hz: 869_650_000,
                max_erp_cdbm: 2699,
                duty_ppm: 100_000,
                window: Duration::from_secs(3600),
            }
        );
        // MeshCore's EU narrow preset sits in g3.
        let narrow = RadioConfig {
            freq_hz: 869_618_000,
            modulation: Modulation::LoRa(LoRa::new(8, Bandwidth::Bw62_5)),
        };
        assert!(r.covers(&narrow));
        assert_eq!(
            applicable_rule(s, &narrow, 2200).map(|i| s[i].duty_ppm),
            Some(100_000)
        );
        assert_eq!(applicable_rule(s, &narrow, 2800), None);
        // 915 MHz: not this profile's business.
        assert!(!r.covers(&cfg(915_000_000)));
        assert!(parse("window_s = 3600\n[[rule]]\nlow_hz = 1\n").is_none());
    }

    #[test]
    fn rule_admission() {
        let r = g1();
        assert!(r.admits(&cfg(868_100_000), 1398));
        assert!(!r.admits(&cfg(868_100_000), 1399));
        assert!(!r.admits(&cfg(868_050_000), 1000), "spills below 868.0 MHz");
        assert!(!r.admits(&cfg(868_550_000), 1000), "spills above 868.6 MHz");
        assert_eq!(r.budget_ns(), 36_000_000_000);
    }

    #[test]
    fn most_permissive_rule_wins() {
        let g4 = |erp, duty| Rule {
            low_hz: 869_700_000,
            high_hz: 870_000_000,
            max_erp_cdbm: erp,
            duty_ppm: duty,
            window: Duration::from_secs(3600),
        };
        let rules = [g1(), g4(1398, 10_000), g4(699, 1_000_000)];
        assert_eq!(applicable_rule(&rules, &cfg(869_850_000), 600), Some(2));
        assert_eq!(applicable_rule(&rules, &cfg(869_850_000), 1000), Some(1));
        assert_eq!(applicable_rule(&rules, &cfg(869_850_000), 2000), None);
        assert_eq!(applicable_rule(&rules, &cfg(868_300_000), 1000), Some(0));
    }

    #[test]
    fn budget_blocks_then_frees() {
        let mut b = DutyBudget::new(&g1(), 0);
        let air = Duration::from_secs(1);
        let mut t = Instant::ZERO;
        let mut sent = 0;
        while b.can_transmit(t, air) {
            b.record(t, air);
            sent += 1;
            t = t + Duration::from_secs(2);
        }
        assert_eq!(sent, 36);
        let next = b.next_opportunity(t, air).unwrap();
        assert!(next > t);
        assert!(b.can_transmit(next, air));
        // The first transmission (t = 0) must have left every one-hour window.
        assert!(next.as_nanos() >= Duration::from_secs(3600).as_nanos() + air.as_nanos());
        assert_eq!(b.next_opportunity(t, Duration::from_secs(120)), None);
    }

    #[test]
    fn margin_reduces_budget() {
        let b = DutyBudget::new(&g1(), 10_000);
        assert_eq!(b.limit_us(), 35_640_000);
    }
}
