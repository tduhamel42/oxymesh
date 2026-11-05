//! Temps en nanosecondes entières (repris de loranet, où ce choix est son ADR-0002). Les opérations saturent au lieu de paniquer.

use core::fmt;
use core::ops::{Add, Sub};

/// Durée en nanosecondes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Duration(u64);

impl Duration {
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(u64::MAX);

    pub const fn from_nanos(ns: u64) -> Self {
        Self(ns)
    }
    pub const fn from_micros(us: u64) -> Self {
        Self(us.saturating_mul(1_000))
    }
    pub const fn from_millis(ms: u64) -> Self {
        Self(ms.saturating_mul(1_000_000))
    }
    pub const fn from_secs(s: u64) -> Self {
        Self(s.saturating_mul(1_000_000_000))
    }
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    pub const fn as_micros(self) -> u64 {
        self.0 / 1_000
    }
    pub const fn as_millis(self) -> u64 {
        self.0 / 1_000_000
    }
    pub const fn checked_add(self, o: Self) -> Option<Self> {
        match self.0.checked_add(o.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }
    pub const fn saturating_add(self, o: Self) -> Self {
        Self(self.0.saturating_add(o.0))
    }
    pub const fn saturating_sub(self, o: Self) -> Self {
        Self(self.0.saturating_sub(o.0))
    }
    pub const fn saturating_mul(self, k: u64) -> Self {
        Self(self.0.saturating_mul(k))
    }
}

impl Add for Duration {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        self.saturating_add(o)
    }
}

impl Sub for Duration {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        self.saturating_sub(o)
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:06}ms", self.0 / 1_000_000, self.0 % 1_000_000)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Instant d'une horloge (locale au nœud, ou temps vrai dans le simulateur), en nanosecondes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Instant(u64);

impl Instant {
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(u64::MAX);

    pub const fn from_nanos(ns: u64) -> Self {
        Self(ns)
    }
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    pub const fn checked_add(self, d: Duration) -> Option<Self> {
        match self.0.checked_add(d.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }
    pub const fn saturating_add(self, d: Duration) -> Self {
        Self(self.0.saturating_add(d.0))
    }
    pub const fn saturating_sub(self, d: Duration) -> Self {
        Self(self.0.saturating_sub(d.0))
    }
    /// Durée écoulée depuis `earlier`, nulle si `earlier` est dans le futur.
    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration(self.0.saturating_sub(earlier.0))
    }
}

impl Add<Duration> for Instant {
    type Output = Self;
    fn add(self, d: Duration) -> Self {
        self.saturating_add(d)
    }
}

impl Sub<Duration> for Instant {
    type Output = Self;
    fn sub(self, d: Duration) -> Self {
        self.saturating_sub(d)
    }
}

impl fmt::Debug for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t+{:?}", Duration(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturating_arithmetic() {
        let t = Instant::from_nanos(u64::MAX - 1);
        assert_eq!(t + Duration::from_secs(1), Instant::MAX);
        assert_eq!(Instant::ZERO - Duration::from_millis(1), Instant::ZERO);
        assert_eq!(
            Instant::from_nanos(5).saturating_duration_since(Instant::from_nanos(9)),
            Duration::ZERO
        );
        assert_eq!(Duration::from_millis(56).as_nanos(), 56_000_000);
    }
}
