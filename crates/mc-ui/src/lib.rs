//! The on-device interface of oxymesh (`docs/spec/ui.md`).
//!
//! `no_std` with `alloc` (ADR-0008): Ratatui keeps its cell buffers on the heap. The protocol
//! crates stay allocation-free. The same code renders on the M5's e-paper (through
//! `mousefood`), in a terminal (`mc ui`) and in snapshot tests.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod demo;
pub mod input;
pub mod panel;
