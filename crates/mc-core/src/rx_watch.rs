//! Is a packet coming in? The C++ firmware's answer (`CustomSX1262::isReceiving`,
//! `src/helpers/radiolib/CustomSX1262.h:108-149`, MeshCore, MIT), which also clears flags the
//! SX126x left latched. The radio raises PREAMBLE_DETECTED and HEADER_VALID and keeps them until
//! they are cleared; a false preamble detection, or a header whose packet never ends, would
//! otherwise read as "receiving" forever: listen before talk would hold every transmission for
//! its 4 s limit, and the noise floor would stop being sampled.
//!
//! - HEADER_ERR: clear the preamble, sync word and header flags; not receiving.
//! - HEADER_VALID: receiving, for at most the airtime of the longest payload; then clear them.
//! - PREAMBLE_DETECTED alone: receiving, for at most the preamble and header time; then clear
//!   PREAMBLE_DETECTED only.
//!
//! The limits come from the radio settings like `RadioLibWrapper::calcMaxPacketMillis`
//! (`RadioLibWrappers.cpp:244-260`).

use crate::radio::LoRa;

/// SX126x IRQ register bits (SX1261/2 datasheet, §13.3.1).
pub const PREAMBLE_DETECTED: u16 = 1 << 2;
pub const SYNC_WORD_VALID: u16 = 1 << 3;
pub const HEADER_VALID: u16 = 1 << 4;
pub const HEADER_ERR: u16 = 1 << 5;

/// What to clear when a header is stale or wrong.
pub const HEADER_FLAGS: u16 = PREAMBLE_DETECTED | SYNC_WORD_VALID | HEADER_VALID | HEADER_ERR;

/// Longest frame (`MAX_TRANS_UNIT`).
const MAX_FRAME: usize = 255;

/// Verdict of one look at the IRQ flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watch {
    pub receiving: bool,
    /// Flags to clear on the radio now (0: none).
    pub clear: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RxWatch {
    preamble_ms: u64,
    payload_ms: u64,
    /// When the current preamble or header was first seen.
    activity_at: Option<u64>,
    header_seen: bool,
}

impl Default for RxWatch {
    fn default() -> Self {
        Self::new()
    }
}

/// `(preamble, payload)` limits in ms for these settings; `cr` is the coding rate denominator
/// (5 to 8), as the C++ passes it.
pub fn limits(lora: &LoRa, cr: u8) -> (u64, u64) {
    // tsym in whole µs, truncated like the C++'s float-to-integer conversion.
    let tsym_us = lora.symbol_ns() / 1000;
    let coeff_x4: u64 = if lora.sf <= 6 { 25 } else { 17 };
    let preamble_us = ((u64::from(lora.preamble) + 8) * 4 + coeff_x4) * tsym_us / 4;
    let total_us = lora.airtime(MAX_FRAME).as_micros();
    let mut payload_us = if total_us > preamble_us {
        total_us - preamble_us
    } else {
        4000u64.saturating_sub(preamble_us)
    };
    if (5..8).contains(&cr) {
        payload_us = payload_us * 8 / u64::from(cr);
    }
    (preamble_us.div_ceil(1000), payload_us.div_ceil(1000))
}

impl RxWatch {
    /// The C++ defaults before the first `setParams`: 66 ms and 3934 ms.
    pub const fn new() -> Self {
        Self {
            preamble_ms: 66,
            payload_ms: 3934,
            activity_at: None,
            header_seen: false,
        }
    }

    /// New radio settings (`CustomSX1262Wrapper::setParams`).
    pub fn configure(&mut self, lora: &LoRa, cr: u8) {
        (self.preamble_ms, self.payload_ms) = limits(lora, cr);
    }

    /// The radio left receive mode (transmission, reconfiguration): forget what was seen.
    pub fn reset(&mut self) {
        self.activity_at = None;
        self.header_seen = false;
    }

    /// One look at the IRQ flags, at `now_ms`.
    pub fn step(&mut self, irq: u16, now_ms: u64) -> Watch {
        const IDLE: Watch = Watch {
            receiving: false,
            clear: 0,
        };
        if irq & HEADER_ERR != 0 {
            self.reset();
            return Watch {
                receiving: false,
                clear: HEADER_FLAGS,
            };
        }
        let header = irq & HEADER_VALID != 0;
        if !header && self.header_seen {
            // Something cleared the header flag (a packet ended): start over.
            self.reset();
            return IDLE;
        }
        if header {
            let since = *self.activity_at.get_or_insert(now_ms);
            if !self.header_seen {
                self.header_seen = true;
                self.activity_at = Some(now_ms);
                return Watch {
                    receiving: true,
                    clear: 0,
                };
            }
            if now_ms.saturating_sub(since) > self.payload_ms {
                self.reset();
                return Watch {
                    receiving: false,
                    clear: HEADER_FLAGS,
                };
            }
            return Watch {
                receiving: true,
                clear: 0,
            };
        }
        if irq & PREAMBLE_DETECTED != 0 {
            let since = *self.activity_at.get_or_insert(now_ms);
            if now_ms.saturating_sub(since) > self.preamble_ms {
                self.activity_at = None;
                return Watch {
                    receiving: false,
                    clear: PREAMBLE_DETECTED,
                };
            }
            return Watch {
                receiving: true,
                clear: 0,
            };
        }
        self.reset();
        IDLE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radio::{Bandwidth, CodingRate};

    fn eu_narrow() -> LoRa {
        let mut l = LoRa::new(8, Bandwidth::Bw62_5);
        l.cr = CodingRate::Cr4_8;
        l.preamble = 32;
        l
    }

    #[test]
    fn limits_at_sf8_62k5() {
        // tsym 4096 µs; preamble ((32 + 8) × 4 + 17) × 4096 / 4 = 181,248 µs.
        let (pre, pay) = limits(&eu_narrow(), 8);
        assert_eq!(pre, 182);
        // 255 bytes at SF8/62.5/CR4-8 with a 32-symbol preamble, minus the preamble time.
        let total = eu_narrow().airtime(255).as_micros();
        assert_eq!(pay, (total - 181_248).div_ceil(1000));
        assert!((2000..2300).contains(&pay), "{pay}");
    }

    #[test]
    fn lower_coding_rate_scales_the_payload_limit_to_cr_8() {
        let mut l = eu_narrow();
        l.cr = CodingRate::Cr4_5;
        let (_, pay5) = limits(&l, 5);
        let total = l.airtime(255).as_micros();
        assert_eq!(pay5, ((total - 181_248) * 8 / 5).div_ceil(1000));
    }

    #[test]
    fn a_stale_preamble_is_cleared() {
        let mut w = RxWatch::new();
        w.configure(&eu_narrow(), 8);
        assert!(w.step(PREAMBLE_DETECTED, 1000).receiving);
        assert!(w.step(PREAMBLE_DETECTED, 1182).receiving);
        let v = w.step(PREAMBLE_DETECTED, 1183);
        assert_eq!(
            v,
            Watch {
                receiving: false,
                clear: PREAMBLE_DETECTED
            }
        );
        // Cleared: the next detection starts its own clock.
        assert!(w.step(PREAMBLE_DETECTED, 1200).receiving);
    }

    #[test]
    fn a_header_counts_for_one_longest_payload() {
        let mut w = RxWatch::new();
        w.configure(&eu_narrow(), 8);
        let (_, pay) = limits(&eu_narrow(), 8);
        assert!(w.step(PREAMBLE_DETECTED, 0).receiving);
        // The header restarts the clock.
        assert!(w.step(PREAMBLE_DETECTED | HEADER_VALID, 150).receiving);
        assert!(
            w.step(PREAMBLE_DETECTED | HEADER_VALID, 150 + pay)
                .receiving
        );
        let v = w.step(PREAMBLE_DETECTED | HEADER_VALID, 151 + pay);
        assert_eq!(
            v,
            Watch {
                receiving: false,
                clear: HEADER_FLAGS
            }
        );
    }

    #[test]
    fn header_error_clears_at_once() {
        let mut w = RxWatch::new();
        let v = w.step(PREAMBLE_DETECTED | HEADER_ERR, 5);
        assert_eq!(v.clear, HEADER_FLAGS);
        assert!(!v.receiving);
    }

    #[test]
    fn a_finished_packet_resets_the_state() {
        let mut w = RxWatch::new();
        assert!(w.step(HEADER_VALID, 0).receiving);
        // RX_DONE handled and the flags cleared by the reader.
        assert!(!w.step(0, 300).receiving);
        // A new header gets a fresh clock.
        assert!(w.step(HEADER_VALID, 10_000).receiving);
        assert!(w.step(HEADER_VALID, 10_000 + 3934).receiving);
    }
}
