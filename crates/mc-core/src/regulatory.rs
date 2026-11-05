//! Règles réglementaires et budget de duty cycle tenu par le nœud (repris de loranet, ADR-0002 et ADR-0006 de loranet ; valeurs dans config/eu868.toml).
//!
//! Les valeurs ne sont jamais écrites ici : elles viennent de `config/regulatory/*.toml`.

use crate::{Duration, Instant, RadioConfig};

/// Un régime d'émission autorisé : plage de fréquences, puissance maximale, duty cycle.
///
/// Une sous-bande à plusieurs régimes (g4 : 5 mW sans limite ou 25 mW à 1 %) donne plusieurs règles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub low_hz: u32,
    pub high_hz: u32,
    /// Puissance apparente rayonnée maximale, en centièmes de dBm.
    pub max_erp_cdbm: i32,
    /// Fraction du temps d'émission autorisée sur la fenêtre, en millionièmes (1 % = 10 000).
    pub duty_ppm: u32,
    /// Fenêtre glissante (1 h dans l'UE).
    pub window: Duration,
}

impl Rule {
    /// L'émission tient-elle entièrement dans la plage, à une puissance autorisée ?
    pub const fn admits(&self, cfg: &RadioConfig, erp_cdbm: i32) -> bool {
        let (lo, hi) = cfg.span_hz();
        lo >= self.low_hz && hi <= self.high_hz && erp_cdbm <= self.max_erp_cdbm
    }

    /// Temps d'émission autorisé par fenêtre, en nanosecondes.
    pub const fn budget_ns(&self) -> u64 {
        ((self.window.as_nanos() as u128 * self.duty_ppm as u128) / 1_000_000) as u64
    }
}

/// Règle applicable à une émission : parmi celles qui l'admettent, la plus permissive.
pub fn applicable_rule(rules: &[Rule], cfg: &RadioConfig, erp_cdbm: i32) -> Option<usize> {
    let mut best: Option<(usize, u32)> = None;
    for (i, r) in rules.iter().enumerate() {
        if r.admits(cfg, erp_cdbm) && best.is_none_or(|(_, d)| r.duty_ppm > d) {
            best = Some((i, r.duty_ppm));
        }
    }
    best.map(|(i, _)| i)
}

/// Nombre de seaux de la fenêtre du budget.
pub const BUCKETS: usize = 62;

/// Budget de duty cycle tenu par le nœud, à seaux, **conservateur** : il ne sous-estime jamais
/// l'usage sur une fenêtre glissante exacte.
///
/// La fenêtre `W` est découpée en seaux de `W / (BUCKETS − 2)`. Une émission est comptée en
/// entier dans le seau de son début ; les `BUCKETS` derniers seaux couvrent toujours au moins
/// `W` plus la durée d'une émission, à condition qu'aucune émission ne dépasse un seau
/// (vérifié par [`DutyBudget::can_transmit`]).
///
/// Les instants passés aux méthodes doivent être **croissants** : interroger un instant futur fait
/// avancer les seaux, et une question posée ensuite sur un instant antérieur verrait un budget
/// vidé. Pour sonder l'avenir, travailler sur une copie (`clone`).
#[derive(Debug, Clone)]
pub struct DutyBudget {
    bucket_ns: u64,
    limit_us: u64,
    used_us: [u32; BUCKETS],
    current: u64,
}

impl DutyBudget {
    /// `margin_ppm` retire une marge de sécurité du budget (10 000 = 1 %), pour couvrir la
    /// dérive d'horloge et les arrondis.
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

    /// Temps d'émission compté sur la fenêtre courante, en microsecondes.
    pub fn used_us(&mut self, now: Instant) -> u64 {
        self.advance(now);
        self.used_us.iter().map(|&u| u64::from(u)).sum()
    }

    /// Budget total par fenêtre, marge déduite, en microsecondes.
    pub const fn limit_us(&self) -> u64 {
        self.limit_us
    }

    /// Une émission de durée `airtime` commençant à `now` respecte-t-elle le budget ?
    pub fn can_transmit(&mut self, now: Instant, airtime: Duration) -> bool {
        airtime.as_nanos() <= self.bucket_ns
            && self.used_us(now) + Self::need_us(airtime) <= self.limit_us
    }

    /// Compte une émission commençant à `now`.
    pub fn record(&mut self, now: Instant, airtime: Duration) {
        self.advance(now);
        let need = u32::try_from(Self::need_us(airtime)).unwrap_or(u32::MAX);
        let cur = self.current;
        if let Some(s) = self.slot(cur) {
            *s = s.saturating_add(need);
        }
    }

    /// Premier instant `≥ now` où une émission de durée `airtime` sera permise, ou `None` si elle
    /// ne le sera jamais (plus longue qu'un seau ou que le budget).
    pub fn next_opportunity(&mut self, now: Instant, airtime: Duration) -> Option<Instant> {
        if airtime.as_nanos() > self.bucket_ns || Self::need_us(airtime) > self.limit_us {
            return None;
        }
        let mut used = self.used_us(now);
        let need = Self::need_us(airtime);
        if used + need <= self.limit_us {
            return Some(now);
        }
        // Au début du seau `current + k`, le seau `current + k − BUCKETS` sort de la fenêtre.
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
        assert!(!r.admits(&cfg(868_050_000), 1000), "déborde sous 868,0 MHz");
        assert!(
            !r.admits(&cfg(868_550_000), 1000),
            "déborde au-dessus de 868,6 MHz"
        );
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
        // La première émission (t = 0) doit avoir quitté toute fenêtre d'une heure.
        assert!(next.as_nanos() >= Duration::from_secs(3600).as_nanos() + air.as_nanos());
        assert_eq!(b.next_opportunity(t, Duration::from_secs(120)), None);
    }

    #[test]
    fn margin_reduces_budget() {
        let b = DutyBudget::new(&g1(), 10_000);
        assert_eq!(b.limit_us(), 35_640_000);
    }
}
