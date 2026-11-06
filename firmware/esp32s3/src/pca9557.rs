//! PCA9557 I/O expander of the ThinkNode M5 (`docs/spec/m5.md` §2), on I2C at 0x18.

use esp_hal::Blocking;
use esp_hal::i2c::master::I2c;

pub const ADDR: u8 = 0x18;

/// Expander pins on the M5.
pub mod pin {
    pub const LED_NOTIFY: u8 = 1;
    pub const LED_ENABLE: u8 = 2;
    pub const LED_POWER: u8 = 3;
    pub const PERIPH_POWER: u8 = 4;
    pub const BACKLIGHT: u8 = 5;
    pub const GPS_VERSION: u8 = 7;
}

const REG_INPUT: u8 = 0;
const REG_OUTPUT: u8 = 1;
const REG_POLARITY: u8 = 2;
const REG_CONFIG: u8 = 3;

pub struct Pca9557<'d> {
    i2c: I2c<'d, Blocking>,
    out: u8,
}

impl<'d> Pca9557<'d> {
    /// IO1 to IO5 as outputs, the others as inputs; normal polarity (the PCA9557 inverts IO4-7
    /// at power-up); peripherals powered, LEDs and backlight off.
    pub fn new(i2c: I2c<'d, Blocking>) -> Result<Self, esp_hal::i2c::master::Error> {
        let mut p = Self {
            i2c,
            out: 1 << pin::PERIPH_POWER,
        };
        p.i2c.write(ADDR, &[REG_POLARITY, 0x00])?;
        p.i2c.write(ADDR, &[REG_OUTPUT, p.out])?;
        p.i2c.write(ADDR, &[REG_CONFIG, 0b1100_0001])?;
        Ok(p)
    }

    pub fn set(&mut self, pin: u8, high: bool) -> Result<(), esp_hal::i2c::master::Error> {
        if high {
            self.out |= 1 << pin;
        } else {
            self.out &= !(1 << pin);
        }
        self.i2c.write(ADDR, &[REG_OUTPUT, self.out])
    }

    /// Full input register (all 8 pins).
    pub fn inputs(&mut self) -> Result<u8, esp_hal::i2c::master::Error> {
        let mut b = [0u8];
        self.i2c.write_read(ADDR, &[REG_INPUT], &mut b)?;
        Ok(b[0])
    }

    pub fn read(&mut self, pin: u8) -> Result<bool, esp_hal::i2c::master::Error> {
        let mut b = [0u8];
        self.i2c.write_read(ADDR, &[REG_INPUT], &mut b)?;
        Ok(b[0] & (1 << pin) != 0)
    }
}
