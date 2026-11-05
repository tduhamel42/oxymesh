//! Trames MeshCore sur l'air (`docs/spec/air.md`) : codec, charges utiles, crypto, hashes,
//! codes de portée, données d'annonce. Compatibles octet pour octet avec MeshCore v1.18.
//!
//! `no_std`, sans allocation, sans flottants ; décodeurs zéro-copie qui ne paniquent jamais.
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

pub mod advert;
pub mod crypto;
pub mod packet;
pub mod payload;

pub use advert::AppData;
pub use crypto::{ChannelSecret, LocalIdentity};
pub use packet::{Header, Packet, PacketBuf, PathLen, PayloadType, Route};
pub use payload::Payload;

/// Erreurs de décodage, de construction et de crypto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Trame ou champ plus court qu'annoncé.
    Truncated,
    /// Version d'en-tête différente de 0.
    Version,
    /// Taille de hash de chemin réservée (4 octets) ou hors de 1..3.
    PathHashSize,
    /// Chemin de plus de 64 octets ou de plus de 63 sauts.
    PathTooLong,
    /// Longueur du chemin incohérente avec `path_len`.
    PathMismatch,
    /// Codes de transport présents sans route qui les porte, ou l'inverse.
    Transport,
    /// Charge utile de plus de 184 octets.
    PayloadTooLong,
    /// Trame de plus de 255 octets.
    FrameTooLong,
    /// Tampon de sortie trop petit.
    Overflow,
    /// Chiffré vide ou non multiple de 16.
    CipherLen,
    /// MAC faux.
    Mac,
    /// Échec de signature.
    Signature,
    /// Texte trop long ou contenant un NUL.
    TextTooLong,
    /// Type ou variante non pris en charge.
    Unsupported,
}
