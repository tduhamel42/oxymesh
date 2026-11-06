//! M5 battery (`docs/spec/m5.md` §3): voltage and percentage from the measured Li-ion curve.

/// Open-circuit voltage from 100 % to 0 % in 10 % steps (hardware facts taken from Meshtastic).
const OCV_MV: [u16; 11] = [
    4100, 4050, 3990, 3890, 3800, 3720, 3630, 3530, 3420, 3300, 3100,
];

/// Percentage (0 to 100) by linear interpolation between two points of the curve.
pub fn percent(mv: u16) -> u8 {
    if mv >= OCV_MV[0] {
        return 100;
    }
    for i in 0..OCV_MV.len() - 1 {
        let (hi, lo) = (OCV_MV[i], OCV_MV[i + 1]);
        if mv >= lo {
            let top = 100 - 10 * i as u32;
            let frac = u32::from(mv - lo) * 10 / u32::from(hi - lo);
            return (top - 10 + frac) as u8;
        }
    }
    0
}

/// Moving average over 8 samples, to avoid jumps on screen.
pub struct Smoother {
    samples: [u16; 8],
    n: usize,
    i: usize,
}

impl Smoother {
    pub const fn new() -> Self {
        Self {
            samples: [0; 8],
            n: 0,
            i: 0,
        }
    }

    pub fn push(&mut self, mv: u16) -> u16 {
        self.samples[self.i] = mv;
        self.i = (self.i + 1) % self.samples.len();
        self.n = (self.n + 1).min(self.samples.len());
        let sum: u32 = self.samples[..self.n].iter().map(|&s| u32::from(s)).sum();
        (sum / self.n as u32) as u16
    }
}

impl Default for Smoother {
    fn default() -> Self {
        Self::new()
    }
}
