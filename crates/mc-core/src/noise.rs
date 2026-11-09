//! Noise floor, the way the C++ firmware measures it (`src/helpers/radiolib/RadioLibWrappers.cpp:
//! 62-105`, MeshCore, MIT): every 2 s (`Dispatcher.cpp:15-16, 66-71`) a new round of 64 RSSI
//! samples starts, taken while listening and no packet is coming in; a sample counts only below
//! the current floor + 14 dB, so a transmission nearby does not lift it. The floor is their mean
//! (truncated, like C), never below −120 dBm. It starts at 0 until the first round ends.

/// Samples per round (`NUM_NOISE_FLOOR_SAMPLES`).
pub const SAMPLES: u8 = 64;
/// A sample counts below floor + this, dB (`SAMPLING_THRESHOLD`).
pub const THRESHOLD: i16 = 14;
/// A new round every (`NOISE_FLOOR_CALIB_INTERVAL`), ms.
pub const ROUND_MS: u64 = 2000;
/// Lowest floor reported.
pub const FLOOR_MIN: i16 = -120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoiseFloor {
    floor: i16,
    n: u8,
    sum: i32,
}

impl Default for NoiseFloor {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseFloor {
    pub const fn new() -> Self {
        Self {
            floor: 0,
            n: 0,
            sum: 0,
        }
    }

    pub fn floor(&self) -> i16 {
        self.floor
    }

    /// After an AGC reset (`RadioLibWrapper::resetAGC`): back to 0, sampling again from scratch, so
    /// a floor stuck low cannot keep rejecting normal samples.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Every [`ROUND_MS`]: a new round, unless one is still sampling (`triggerNoiseFloorCalibrate`).
    pub fn trigger(&mut self) {
        if self.n >= SAMPLES {
            self.n = 0;
            self.sum = 0;
        }
    }

    /// On every pass of the main loop (`RadioLibWrapper::loop`): `rssi` reads the instantaneous
    /// RSSI, only called while listening with no packet coming in.
    pub fn step(&mut self, listening: bool, receiving: bool, rssi: impl FnOnce() -> Option<i16>) {
        if listening && self.n < SAMPLES {
            if !receiving
                && let Some(r) = rssi()
                && r < self.floor.saturating_add(THRESHOLD)
            {
                self.n += 1;
                self.sum += i32::from(r);
            }
        } else if self.n >= SAMPLES && self.sum != 0 {
            let mean = self.sum / i32::from(SAMPLES);
            self.floor = i16::try_from(mean).unwrap_or(FLOOR_MIN).max(FLOOR_MIN);
            self.sum = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One full round at `rssi`, then the pass that computes the floor.
    fn round(nf: &mut NoiseFloor, rssi: impl Fn(u8) -> i16) {
        nf.trigger();
        for i in 0..200u8 {
            nf.step(true, false, || Some(rssi(i)));
        }
        nf.step(true, false, || unreachable!("the round is full"));
    }

    #[test]
    fn mean_of_quiet_samples() {
        let mut nf = NoiseFloor::new();
        assert_eq!(nf.floor(), 0);
        round(&mut nf, |i| if i % 2 == 0 { -110 } else { -111 });
        // −7072 / 64 = −110.5, truncated towards zero like C.
        assert_eq!(nf.floor(), -110);
        // A transmission nearby (−60) is above floor + 14: ignored.
        round(&mut nf, |i| if i % 3 == 0 { -60 } else { -105 });
        assert_eq!(nf.floor(), -105);
        // Never below −120.
        round(&mut nf, |_| -130);
        assert_eq!(nf.floor(), -120);
    }

    #[test]
    fn no_samples_while_receiving_or_sending() {
        let mut nf = NoiseFloor::new();
        for _ in 0..100 {
            nf.step(true, true, || unreachable!("a packet is coming in"));
            nf.step(false, false, || unreachable!("not listening"));
        }
        assert_eq!(nf.floor(), 0);
        // A trigger in the middle of a round does not restart it.
        for _ in 0..10 {
            nf.step(true, false, || Some(-100));
        }
        nf.trigger();
        for _ in 0..54 {
            nf.step(true, false, || Some(-100));
        }
        nf.step(true, false, || None);
        assert_eq!(nf.floor(), -100);
    }
}
