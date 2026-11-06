//! Protocole compagnon MeshCore (`docs/spec/companion.md`) : trames de l'app et de l'appareil,
//! et tramage USB. Base reprise de `ln-meshcore` (loranet, côté app), étendue côté appareil.
//!
//! `no_std`, sans allocation ; décodage zéro-copie. Deux modes pour les commandes (ADR-0002) :
//! [`Command::decode`] strict et canonique, [`Command::decode_device`] comme le firmware C++.
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

pub mod codec;
pub mod command;
pub mod response;
pub mod usb;

pub use codec::{DecodeError, DeviceReject, FRAME_MAX, cmd, err, resp};
pub use command::{Command, FloodScopeKey};
pub use response::{Contact, DeviceInfo, LoginServer, Response, SelfInfo, Stats, freq_ranges};
pub use usb::{FROM_DEVICE, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame};
