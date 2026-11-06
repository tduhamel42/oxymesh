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
