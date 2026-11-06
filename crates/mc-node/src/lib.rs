//! The IO-free state machine of a MeshCore companion (ADR-0003): dispatcher, mesh, contacts,
//! messages, companion. The runner (firmware, simulator, virtual device) feeds it received
//! frames, app frames, the time and randomness; it returns frames to transmit, frames for the
//! app and writes to persist.
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

pub mod chat;
pub mod clock;
pub mod contacts;
pub mod device;
pub mod dispatch;
pub mod node;
pub mod persist;
pub mod requests;
pub mod rx;
pub mod tables;

pub use device::{Action, Device, Info, Storage};
pub use node::{Event, Node, Prefs, SendError, Sent};
