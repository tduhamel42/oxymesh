//! Modulations and airtime in exact integers (taken from loranet, where this choice is its
//! ADR-0002).
//!
//! LoRa: Semtech AN1200.13 formula for SF7 to SF12, SX1261/2 datasheet variant (§6.1.4) for SF5
//! and SF6. FSK: transmitted bits divided by the bit rate.

use crate::Duration;

/// SX126x LoRa bandwidths, with their exact value in Hz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bandwidth {
    Bw7_8,
    Bw10_4,
    Bw15_6,
    Bw20_8,
    Bw31_25,
    Bw41_7,
    Bw62_5,
    Bw125,
    Bw250,
    Bw500,
}

impl Bandwidth {
    /// All bandwidths, from narrowest to widest.
    pub const ALL: [Bandwidth; 10] = [
        Bandwidth::Bw7_8,
        Bandwidth::Bw10_4,
        Bandwidth::Bw15_6,
        Bandwidth::Bw20_8,
        Bandwidth::Bw31_25,
        Bandwidth::Bw41_7,
        Bandwidth::Bw62_5,
        Bandwidth::Bw125,
        Bandwidth::Bw250,
        Bandwidth::Bw500,
    ];

    /// Exact bandwidth in Hz as `(numerator, denominator)`.
    pub const fn hz_ratio(self) -> (u64, u64) {
        match self {
            Bandwidth::Bw7_8 => (15_625, 2),
            Bandwidth::Bw10_4 => (31_250, 3),
            Bandwidth::Bw15_6 => (15_625, 1),
            Bandwidth::Bw20_8 => (62_500, 3),
            Bandwidth::Bw31_25 => (31_250, 1),
            Bandwidth::Bw41_7 => (125_000, 3),
            Bandwidth::Bw62_5 => (62_500, 1),
            Bandwidth::Bw125 => (125_000, 1),
            Bandwidth::Bw250 => (250_000, 1),
            Bandwidth::Bw500 => (500_000, 1),
        }
    }

    /// Bandwidth rounded down to the Hz (for channel overlap checks).
    pub const fn hz_floor(self) -> u32 {
        let (n, d) = self.hz_ratio();
        (n / d) as u32
    }

    /// Bandwidth whose nominal value in Hz is closest to `hz`.
    pub fn from_hz(hz: u32) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|b| b.hz_floor().abs_diff(hz) <= 1)
    }
}

/// LoRa coding rate, 4/5 to 4/8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CodingRate {
    Cr4_5,
    Cr4_6,
    Cr4_7,
    Cr4_8,
}

impl CodingRate {
    /// `CR` value in the formula (1 to 4).
    pub const fn value(self) -> u64 {
        match self {
            CodingRate::Cr4_5 => 1,
            CodingRate::Cr4_6 => 2,
            CodingRate::Cr4_7 => 3,
            CodingRate::Cr4_8 => 4,
        }
    }
}

/// Low data rate optimisation (LDRO) setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ldro {
    /// Enabled when the symbol duration exceeds 16 ms, as Semtech recommends.
    Auto,
    On,
    Off,
}

/// LoRa modulation parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoRa {
    /// Spreading factor, 5 to 12.
    pub sf: u8,
    pub bw: Bandwidth,
    pub cr: CodingRate,
    /// Programmed preamble length, in symbols.
    pub preamble: u16,
    pub explicit_header: bool,
    pub crc: bool,
    pub ldro: Ldro,
}

impl LoRa {
    /// Project default: preamble 8, explicit header, CRC, automatic LDRO.
    pub const fn new(sf: u8, bw: Bandwidth) -> Self {
        Self {
            sf,
            bw,
            cr: CodingRate::Cr4_5,
            preamble: 8,
            explicit_header: true,
            crc: true,
            ldro: Ldro::Auto,
        }
    }

    /// Symbol duration in nanoseconds, rounded up.
    pub const fn symbol_ns(&self) -> u64 {
        let (num, den) = self.bw.hz_ratio();
        let t = (1u128 << self.sf) * 1_000_000_000 * den as u128;
        t.div_ceil(num as u128) as u64
    }

    /// Whether LDRO is actually applied.
    pub const fn ldro_on(&self) -> bool {
        match self.ldro {
            Ldro::On => true,
            Ldro::Off => false,
            Ldro::Auto => self.symbol_ns() > 16_000_000,
        }
    }

    /// Number of symbols in the frame, in quarter symbols (the preamble adds 4.25 or 6.25 more).
    pub const fn quarter_symbols(&self, payload_len: usize) -> u64 {
        let pl = payload_len as i64;
        let sf = self.sf as i64;
        let cr = self.cr.value() as i64;
        let crc = if self.crc { 16 } else { 0 };
        let (num, den, extra_quarters) = if sf <= 6 {
            // SX126x, SF5/SF6: no "+8", preamble + 6.25.
            let header = if self.explicit_header { 20 } else { 0 };
            (8 * pl + crc - 4 * sf + header, 4 * sf, 25)
        } else {
            let ih = if self.explicit_header { 0 } else { 1 };
            let de = if self.ldro_on() { 1 } else { 0 };
            (8 * pl - 4 * sf + 28 + crc - 20 * ih, 4 * (sf - 2 * de), 17)
        };
        let blocks = if num > 0 { (num + den - 1) / den } else { 0 };
        let n_payload = 8 + blocks * (cr + 4);
        (4 * self.preamble as i64 + extra_quarters + 4 * n_payload) as u64
    }

    /// Airtime of a `payload_len`-byte frame, rounded up to the nanosecond.
    pub const fn airtime(&self, payload_len: usize) -> Duration {
        let (num, den) = self.bw.hz_ratio();
        let q = self.quarter_symbols(payload_len) as u128;
        let t = q * (1u128 << self.sf) * 1_000_000_000 * den as u128;
        let d = 4 * num as u128;
        Duration::from_nanos(t.div_ceil(d) as u64)
    }
}

/// FSK modulation parameters (variable-length packets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fsk {
    pub bitrate_bps: u32,
    /// Frequency deviation in Hz.
    pub fdev_hz: u32,
    pub preamble_bytes: u16,
    pub sync_bytes: u8,
    /// Length byte (1) in variable-length packets.
    pub header_bytes: u8,
    pub crc_bytes: u8,
}

impl Fsk {
    /// Default: 5-byte preamble, 3-byte sync word, 1-byte length, 2-byte CRC.
    pub const fn new(bitrate_bps: u32, fdev_hz: u32) -> Self {
        Self {
            bitrate_bps,
            fdev_hz,
            preamble_bytes: 5,
            sync_bytes: 3,
            header_bytes: 1,
            crc_bytes: 2,
        }
    }

    /// Occupied bandwidth approximated by Carson's rule: `2·(fdev + bitrate/2)`.
    pub const fn occupied_hz(&self) -> u32 {
        2 * self.fdev_hz + self.bitrate_bps
    }

    pub const fn airtime(&self, payload_len: usize) -> Duration {
        let bytes = self.preamble_bytes as u128
            + self.sync_bytes as u128
            + self.header_bytes as u128
            + payload_len as u128
            + self.crc_bytes as u128;
        let t = bytes * 8 * 1_000_000_000;
        Duration::from_nanos(t.div_ceil(self.bitrate_bps as u128) as u64)
    }
}

/// Modulation of a transmission or a reception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modulation {
    LoRa(LoRa),
    Fsk(Fsk),
}

impl Modulation {
    pub const fn airtime(&self, payload_len: usize) -> Duration {
        match self {
            Modulation::LoRa(l) => l.airtime(payload_len),
            Modulation::Fsk(f) => f.airtime(payload_len),
        }
    }

    /// Occupied bandwidth in Hz, rounded down to the Hz.
    pub const fn occupied_hz(&self) -> u32 {
        match self {
            Modulation::LoRa(l) => l.bw.hz_floor(),
            Modulation::Fsk(f) => f.occupied_hz(),
        }
    }

    /// Can the two settings demodulate each other? (same modulation, same waveform parameters)
    pub const fn compatible(&self, other: &Modulation) -> bool {
        match (self, other) {
            (Modulation::LoRa(a), Modulation::LoRa(b)) => {
                a.sf == b.sf
                    && a.bw as u8 == b.bw as u8
                    && a.explicit_header == b.explicit_header
                    && a.ldro_on() == b.ldro_on()
            }
            (Modulation::Fsk(a), Modulation::Fsk(b)) => {
                a.bitrate_bps == b.bitrate_bps && a.fdev_hz == b.fdev_hz
            }
            _ => false,
        }
    }
}

/// Complete radio setting: centre frequency and modulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RadioConfig {
    pub freq_hz: u32,
    pub modulation: Modulation,
}

impl RadioConfig {
    /// Bounds of the occupied band `[low, high]` in Hz.
    pub const fn span_hz(&self) -> (u32, u32) {
        let half = self.modulation.occupied_hz() / 2;
        (
            self.freq_hz.saturating_sub(half),
            self.freq_hz.saturating_add(half),
        )
    }

    /// Do the two settings occupy overlapping frequencies?
    pub const fn overlaps(&self, other: &RadioConfig) -> bool {
        let (a0, a1) = self.span_hz();
        let (b0, b1) = other.span_hz();
        a0 < b1 && b0 < a1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_md_vectors() {
        // BW 125 kHz, CR 4/5, preamble 8, explicit header, CRC, 20 bytes.
        let sf7 = LoRa::new(7, Bandwidth::Bw125);
        assert!(!sf7.ldro_on());
        assert_eq!(sf7.airtime(20), Duration::from_micros(56_576));
        let sf12 = LoRa::new(12, Bandwidth::Bw125);
        assert!(sf12.ldro_on());
        assert_eq!(sf12.airtime(20), Duration::from_micros(1_318_912));
    }

    #[test]
    fn ldro_threshold() {
        assert!(LoRa::new(11, Bandwidth::Bw125).ldro_on()); // 16.384 ms
        assert!(!LoRa::new(10, Bandwidth::Bw125).ldro_on()); // 8.192 ms
        assert!(LoRa::new(12, Bandwidth::Bw250).ldro_on());
        assert!(!LoRa::new(11, Bandwidth::Bw250).ldro_on());
        assert!(LoRa::new(10, Bandwidth::Bw62_5).ldro_on());
    }

    #[test]
    fn sf5_sf6_variant() {
        // Computed by hand from the SX1261/2 datasheet §6.1.4 (to cross-check with the Semtech
        // calculator). SF5, BW 125, CR 4/5, preamble 8, explicit header, CRC, 20 bytes:
        // preamble 8 + 6.25; payload 8 + ceil((160 + 16 - 20 + 20) / 20) * 5 = 8 + 9*5 = 53
        // total 67.25 symbols of 256 µs = 17.216 ms.
        let sf5 = LoRa::new(5, Bandwidth::Bw125);
        assert_eq!(sf5.airtime(20), Duration::from_micros(17_216));
        // SF6: payload 8 + ceil((160 + 16 - 24 + 20) / 24) * 5 = 8 + 8*5 = 48; total 62.25 × 512 µs.
        let sf6 = LoRa::new(6, Bandwidth::Bw125);
        assert_eq!(sf6.airtime(20), Duration::from_micros(31_872));
    }

    #[test]
    fn fractional_bandwidths_are_exact() {
        // 10.4 kHz = 31250/3 Hz: T_sym(SF7) = 128 · 3 / 31250 s = 12.288 ms exactly.
        let l = LoRa::new(7, Bandwidth::Bw10_4);
        assert_eq!(l.symbol_ns(), 12_288_000);
        assert_eq!(Bandwidth::from_hz(125_000), Some(Bandwidth::Bw125));
        assert_eq!(Bandwidth::from_hz(41_666), Some(Bandwidth::Bw41_7));
        assert_eq!(Bandwidth::from_hz(100_000), None);
    }

    #[test]
    fn implicit_header_and_no_crc_are_shorter() {
        // At 31 bytes, SF9: 8 blocks with an explicit header, 7 without (at 30 bytes, 7 in both cases).
        let mut l = LoRa::new(9, Bandwidth::Bw125);
        let base = l.airtime(31);
        l.explicit_header = false;
        assert!(l.airtime(31) < base);
        l.crc = false;
        assert!(l.airtime(31) < base);
        assert_eq!(
            LoRa::new(7, Bandwidth::Bw125).quarter_symbols(0),
            4 * 8 + 17 + 4 * (8 + 5)
        );
    }

    #[test]
    fn fsk_airtime() {
        // 50 kbit/s, 5 + 3 + 1 + 20 + 2 = 31 bytes = 248 bits → 4.96 ms.
        assert_eq!(
            Fsk::new(50_000, 25_000).airtime(20),
            Duration::from_micros(4_960)
        );
    }

    #[test]
    fn config_overlap() {
        let lora = |f| RadioConfig {
            freq_hz: f,
            modulation: Modulation::LoRa(LoRa::new(7, Bandwidth::Bw125)),
        };
        assert!(lora(868_100_000).overlaps(&lora(868_150_000)));
        assert!(!lora(868_100_000).overlaps(&lora(868_300_000)));
        assert!(!lora(868_100_000).overlaps(&lora(868_225_000)));
    }
}
