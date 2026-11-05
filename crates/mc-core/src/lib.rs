//! Types communs à toutes les crates du cœur (`no_std`, sans allocation, sans flottants).
//!
//! Le contenu arrive à l'étape MC1 (`docs/spec/air.md`).
#![no_std]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::float_arithmetic
)]
