//! MeshCore-compatible firmware for the ESP32-S3: drivers and boards.
#![no_std]

pub mod battery;
pub mod ble;
pub mod buzzer;
pub mod flashstore;
pub mod pca9557;
pub mod pcf8563;
pub mod ssd1681;
pub mod sx126x;
pub mod tsens;

/// ThinkNode M5 (`docs/spec/m5.md`).
pub mod m5 {
    /// Radio TCXO, on DIO3.
    pub const TCXO_MILLIVOLTS: u16 = 3300;
    /// PA current limit (`SX126X_CURRENT_LIMIT`).
    pub const OCP_MA: u16 = 140;
    pub const TX_POWER_MAX: i8 = 22;
    /// Battery measurement divider (2.0 in Meshtastic, 2.11 in MeshCore): to be calibrated.
    pub const BATTERY_DIVIDER_MILLI: u32 = 2000;
}
