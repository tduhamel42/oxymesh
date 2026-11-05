//! Types communs à toutes les crates du cœur : lecture et écriture d'octets sans panic, temps en
//! nanosecondes entières, modulations et temps d'antenne exacts, budget de duty cycle.
//!
//! Repris de `ln-core` (loranet). `no_std`, sans allocation, sans flottants.
#![no_std]
#![deny(clippy::float_arithmetic)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod bytes;
pub mod hex;
pub mod radio;
pub mod regulatory;
pub mod time;

pub use bytes::{Overflow, Reader, Truncated, Writer};
pub use hex::Hex;
pub use radio::{Bandwidth, CodingRate, Fsk, Ldro, LoRa, Modulation, RadioConfig};
pub use regulatory::{DutyBudget, Rule, applicable_rule};
pub use time::{Duration, Instant};

/// Trame radio la plus longue (SX126x, et `MAX_TRANS_UNIT` de MeshCore).
pub const FRAME_MAX: usize = 255;
