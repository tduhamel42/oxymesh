//! Real-time clock (Unix seconds) on top of the runner's monotonic clock.

use mc_core::{Duration, Instant};

/// Current Unix time, derived from a sync point and the monotonic clock.
#[derive(Debug, Clone)]
pub struct Rtc {
    base_epoch: u32,
    base: Instant,
    last_unique: u32,
}

impl Rtc {
    pub const fn new(epoch: u32, now: Instant) -> Self {
        Self {
            base_epoch: epoch,
            base: now,
            last_unique: 0,
        }
    }

    pub fn now(&self, now: Instant) -> u32 {
        let secs = now.saturating_duration_since(self.base).as_nanos() / 1_000_000_000;
        self.base_epoch
            .saturating_add(u32::try_from(secs).unwrap_or(u32::MAX))
    }

    pub fn set(&mut self, now: Instant, epoch: u32) {
        self.base_epoch = epoch;
        self.base = now;
    }

    /// Time strictly increasing from one call to the next (`getCurrentTimeUnique`, MeshCore.h:119).
    pub fn unique(&mut self, now: Instant) -> u32 {
        let t = self.now(now);
        if t <= self.last_unique {
            self.last_unique = self.last_unique.wrapping_add(1);
        } else {
            self.last_unique = t;
        }
        self.last_unique
    }
}

/// Milliseconds since the origin of the monotonic clock (for C++-style durations).
pub const fn ms(d: Duration) -> u64 {
    d.as_millis()
}
