//! Protocole compagnon MeshCore (`docs/spec/companion.md`) : trames de l'app et de l'appareil,
//! et tramage USB. Base reprise de `ln-meshcore` (loranet, côté app), étendue côté appareil.
//!
//! `no_std`, sans allocation ; décodage zéro-copie et canonique.
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
pub mod usb;

pub use codec::{
    Command, Contact, DecodeError, DeviceInfo, FRAME_MAX, Response, SelfInfo, cmd, err, resp,
};
pub use usb::{FROM_DEVICE, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame};
