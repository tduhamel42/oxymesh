//! Extenseur d'E/S PCA9557 du ThinkNode M5 (`docs/spec/m5.md` §2), sur I2C en 0x18.

use esp_hal::Blocking;
use esp_hal::i2c::master::I2c;

pub const ADDR: u8 = 0x18;

/// Broches de l'extenseur sur le M5.
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
    /// IO1 à IO5 en sortie, les autres en entrée ; polarité normale (le PCA9557 inverse IO4-7
    /// à la mise sous tension) ; périphériques alimentés, LED et éclairage éteints.
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

    pub fn read(&mut self, pin: u8) -> Result<bool, esp_hal::i2c::master::Error> {
        let mut b = [0u8];
        self.i2c.write_read(ADDR, &[REG_INPUT], &mut b)?;
        Ok(b[0] & (1 << pin) != 0)
    }
}
