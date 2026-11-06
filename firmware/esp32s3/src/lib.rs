//! Firmware compatible MeshCore pour ESP32-S3 : pilotes et cartes.
#![no_std]

pub mod battery;
pub mod flashstore;
pub mod pca9557;
pub mod ssd1681;
pub mod sx126x;

/// ThinkNode M5 (`docs/spec/m5.md`).
pub mod m5 {
    /// TCXO de la radio, sur DIO3.
    pub const TCXO_MILLIVOLTS: u16 = 3300;
    /// Limite de courant du PA (`SX126X_CURRENT_LIMIT`).
    pub const OCP_MA: u16 = 140;
    pub const TX_POWER_MAX: i8 = 22;
    /// Diviseur de la mesure de batterie (2,0 chez Meshtastic, 2,11 chez MeshCore) : à étalonner.
    pub const BATTERY_DIVIDER_MILLI: u32 = 2000;
}
