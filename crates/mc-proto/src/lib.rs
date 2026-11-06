//! MeshCore companion protocol (`docs/spec/companion.md`): app and device frames, USB
//! framing and the Bluetooth link's queues. Base taken from `ln-meshcore` (loranet, app
//! side), extended on the device side.
//!
//! `no_std`, no allocation; zero-copy decoding. Two modes for commands (ADR-0002):
//! [`Command::decode`], strict and canonical, and [`Command::decode_device`], like the C++
//! firmware.
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

pub mod ble;
pub mod codec;
pub mod command;
pub mod response;
pub mod usb;

pub use codec::{DecodeError, DeviceReject, FRAME_MAX, cmd, err, resp};
pub use command::{Command, FloodScopeKey};
pub use response::{Contact, DeviceInfo, LoginServer, Response, SelfInfo, Stats, freq_ranges};
pub use usb::{FROM_DEVICE, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame};
