//! La machine sans IO d'un compagnon MeshCore (ADR-0003) : dispatcher, mesh, contacts,
//! messages, compagnon. Le runner (firmware, simulateur, appareil virtuel) lui passe les trames
//! reçues, les trames de l'app, l'heure et l'aléa ; elle rend les trames à émettre, les trames
//! vers l'app et les écritures à persister.
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
pub mod dispatch;
pub mod node;
pub mod persist;
pub mod rx;
pub mod tables;

pub use node::{Event, Node, Prefs, SendError, Sent};
