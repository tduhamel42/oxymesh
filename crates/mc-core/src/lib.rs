//! Types shared by all core crates: panic-free byte reading and writing, time in integer
//! nanoseconds, modulations and exact airtime, duty-cycle budget.
//!
//! Taken from `ln-core` (loranet). `no_std`, no allocation, no floats.
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
pub mod civil;
pub mod hex;
pub mod noise;
pub mod radio;
pub mod regulatory;
pub mod time;

pub use bytes::{Overflow, Reader, Truncated, Writer};
pub use hex::Hex;
pub use radio::{Bandwidth, CodingRate, Fsk, Ldro, LoRa, Modulation, RadioConfig};
pub use regulatory::{DutyBudget, Rule, applicable_rule};
pub use time::{Duration, Instant};

/// Longest radio frame (SX126x, and MeshCore's `MAX_TRANS_UNIT`).
pub const FRAME_MAX: usize = 255;
