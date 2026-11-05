//! Modulations et temps d'antenne en entiers exacts (repris de loranet, où ce choix est son ADR-0002).
//!
//! LoRa : formule Semtech AN1200.13 pour SF7 à SF12, variante de la datasheet SX1261/2
//! (§6.1.4) pour SF5 et SF6. FSK : bits émis divisés par le débit.

use crate::Duration;

/// Largeurs de bande LoRa du SX126x, avec leur valeur exacte en Hz.
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
    /// Toutes les largeurs, de la plus étroite à la plus large.
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

    /// Largeur exacte en Hz sous forme `(numérateur, dénominateur)`.
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

    /// Largeur arrondie au Hz inférieur (pour les recouvrements de canaux).
    pub const fn hz_floor(self) -> u32 {
        let (n, d) = self.hz_ratio();
        (n / d) as u32
    }

    /// Largeur dont la valeur nominale en Hz est la plus proche de `hz`.
    pub fn from_hz(hz: u32) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|b| b.hz_floor().abs_diff(hz) <= 1)
    }
}

/// Rendement de codage LoRa 4/5 à 4/8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CodingRate {
    Cr4_5,
    Cr4_6,
    Cr4_7,
    Cr4_8,
}

impl CodingRate {
    /// Valeur `CR` de la formule (1 à 4).
    pub const fn value(self) -> u64 {
        match self {
            CodingRate::Cr4_5 => 1,
            CodingRate::Cr4_6 => 2,
            CodingRate::Cr4_7 => 3,
            CodingRate::Cr4_8 => 4,
        }
    }
}

/// Réglage de l'optimisation bas débit (LDRO).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ldro {
    /// Activée quand la durée de symbole dépasse 16 ms, comme le recommande Semtech.
    Auto,
    On,
    Off,
}

/// Paramètres d'une modulation LoRa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoRa {
    /// Facteur d'étalement, 5 à 12.
    pub sf: u8,
    pub bw: Bandwidth,
    pub cr: CodingRate,
    /// Longueur du préambule programmé, en symboles.
    pub preamble: u16,
    pub explicit_header: bool,
    pub crc: bool,
    pub ldro: Ldro,
}

impl LoRa {
    /// Réglage par défaut du projet : préambule 8, en-tête explicite, CRC, LDRO automatique.
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

    /// Durée d'un symbole en nanosecondes, arrondie vers le haut.
    pub const fn symbol_ns(&self) -> u64 {
        let (num, den) = self.bw.hz_ratio();
        let t = (1u128 << self.sf) * 1_000_000_000 * den as u128;
        t.div_ceil(num as u128) as u64
    }

    /// LDRO effectivement appliquée.
    pub const fn ldro_on(&self) -> bool {
        match self.ldro {
            Ldro::On => true,
            Ldro::Off => false,
            Ldro::Auto => self.symbol_ns() > 16_000_000,
        }
    }

    /// Nombre de symboles de la trame, en quarts de symbole (le préambule en compte 4,25 ou 6,25 de plus).
    pub const fn quarter_symbols(&self, payload_len: usize) -> u64 {
        let pl = payload_len as i64;
        let sf = self.sf as i64;
        let cr = self.cr.value() as i64;
        let crc = if self.crc { 16 } else { 0 };
        let (num, den, extra_quarters) = if sf <= 6 {
            // SX126x, SF5/SF6 : pas de « +8 », préambule + 6,25.
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

    /// Temps d'antenne d'une trame de `payload_len` octets, arrondi vers le haut à la nanoseconde.
    pub const fn airtime(&self, payload_len: usize) -> Duration {
        let (num, den) = self.bw.hz_ratio();
        let q = self.quarter_symbols(payload_len) as u128;
        let t = q * (1u128 << self.sf) * 1_000_000_000 * den as u128;
        let d = 4 * num as u128;
        Duration::from_nanos(t.div_ceil(d) as u64)
    }
}

/// Paramètres d'une modulation FSK (paquets à longueur variable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fsk {
    pub bitrate_bps: u32,
    /// Déviation de fréquence en Hz.
    pub fdev_hz: u32,
    pub preamble_bytes: u16,
    pub sync_bytes: u8,
    /// Octet de longueur (1) en paquets variables.
    pub header_bytes: u8,
    pub crc_bytes: u8,
}

impl Fsk {
    /// Réglage par défaut : préambule 5 octets, synchro 3 octets, longueur 1 octet, CRC 2 octets.
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

    /// Largeur occupée approchée par la règle de Carson : `2·(fdev + débit/2)`.
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

/// Modulation d'une émission ou d'une écoute.
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

    /// Largeur occupée en Hz, arrondie au Hz inférieur.
    pub const fn occupied_hz(&self) -> u32 {
        match self {
            Modulation::LoRa(l) => l.bw.hz_floor(),
            Modulation::Fsk(f) => f.occupied_hz(),
        }
    }

    /// Deux réglages se démodulent-ils l'un l'autre ? (même modulation, mêmes paramètres de forme d'onde)
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

/// Réglage radio complet : fréquence centrale et modulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RadioConfig {
    pub freq_hz: u32,
    pub modulation: Modulation,
}

impl RadioConfig {
    /// Bornes de la bande occupée `[bas, haut]` en Hz.
    pub const fn span_hz(&self) -> (u32, u32) {
        let half = self.modulation.occupied_hz() / 2;
        (
            self.freq_hz.saturating_sub(half),
            self.freq_hz.saturating_add(half),
        )
    }

    /// Les deux réglages occupent-ils des fréquences qui se recouvrent ?
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
        // BW 125 kHz, CR 4/5, préambule 8, en-tête explicite, CRC, 20 octets.
        let sf7 = LoRa::new(7, Bandwidth::Bw125);
        assert!(!sf7.ldro_on());
        assert_eq!(sf7.airtime(20), Duration::from_micros(56_576));
        let sf12 = LoRa::new(12, Bandwidth::Bw125);
        assert!(sf12.ldro_on());
        assert_eq!(sf12.airtime(20), Duration::from_micros(1_318_912));
    }

    #[test]
    fn ldro_threshold() {
        assert!(LoRa::new(11, Bandwidth::Bw125).ldro_on()); // 16,384 ms
        assert!(!LoRa::new(10, Bandwidth::Bw125).ldro_on()); // 8,192 ms
        assert!(LoRa::new(12, Bandwidth::Bw250).ldro_on());
        assert!(!LoRa::new(11, Bandwidth::Bw250).ldro_on());
        assert!(LoRa::new(10, Bandwidth::Bw62_5).ldro_on());
    }

    #[test]
    fn sf5_sf6_variant() {
        // Calcul à la main depuis la datasheet SX1261/2 §6.1.4 (à recouper avec le calculateur Semtech).
        // SF5, BW 125, CR 4/5, préambule 8, en-tête explicite, CRC, 20 octets :
        // préambule 8 + 6,25 ; charge 8 + ceil((160 + 16 - 20 + 20) / 20) * 5 = 8 + 9*5 = 53
        // total 67,25 symboles de 256 µs = 17,216 ms.
        let sf5 = LoRa::new(5, Bandwidth::Bw125);
        assert_eq!(sf5.airtime(20), Duration::from_micros(17_216));
        // SF6 : charge 8 + ceil((160 + 16 - 24 + 20) / 24) * 5 = 8 + 8*5 = 48 ; total 62,25 × 512 µs.
        let sf6 = LoRa::new(6, Bandwidth::Bw125);
        assert_eq!(sf6.airtime(20), Duration::from_micros(31_872));
    }

    #[test]
    fn fractional_bandwidths_are_exact() {
        // 10,4 kHz = 31250/3 Hz : T_sym(SF7) = 128 · 3 / 31250 s = 12,288 ms exactement.
        let l = LoRa::new(7, Bandwidth::Bw10_4);
        assert_eq!(l.symbol_ns(), 12_288_000);
        assert_eq!(Bandwidth::from_hz(125_000), Some(Bandwidth::Bw125));
        assert_eq!(Bandwidth::from_hz(41_666), Some(Bandwidth::Bw41_7));
        assert_eq!(Bandwidth::from_hz(100_000), None);
    }

    #[test]
    fn implicit_header_and_no_crc_are_shorter() {
        // À 31 octets, SF9 : 8 blocs avec en-tête explicite, 7 sans (à 30 octets, 7 dans les deux cas).
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
        // 50 kbit/s, 5 + 3 + 1 + 20 + 2 = 31 octets = 248 bits → 4,96 ms.
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
