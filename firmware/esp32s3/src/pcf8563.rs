//! PCF8563 real-time clock of the ThinkNode M5 (`docs/spec/m5.md` §2), on the main I2C bus
//! (SDA 2, SCL 1) at 0x51. Kept in UTC, like the C++ firmware (`AutoDiscoverRTCClock` with
//! RTClib's `RTC_PCF8563`: years 2000-2099, the century bit unused), so both read the same
//! time after a change of firmware.

use esp_hal::Blocking;
use esp_hal::i2c::master::{Error, I2c};
use mc_core::civil::{Civil, from_bcd, to_bcd};

pub const ADDR: u8 = 0x51;
/// Control/status 1; the time registers follow at 0x02.
const REG_CONTROL1: u8 = 0x00;
const REG_SECONDS: u8 = 0x02;
/// In the seconds register: the oscillator stopped, the time is not to be trusted.
const VL: u8 = 0x80;

pub struct Pcf8563<'d> {
    i2c: I2c<'d, Blocking>,
}

impl<'d> Pcf8563<'d> {
    pub fn new(i2c: I2c<'d, Blocking>) -> Self {
        Self { i2c }
    }

    /// Unix time, or `None` when the chip does not answer, lost power or holds garbage.
    pub fn read(&mut self) -> Option<u32> {
        let mut b = [0u8; 7];
        self.i2c.write_read(ADDR, &[REG_SECONDS], &mut b).ok()?;
        let [s, min, h, d, wd, mo, y] = b;
        if s & VL != 0 {
            return None;
        }
        Civil {
            year: 2000 + i32::from(from_bcd(y)?),
            month: from_bcd(mo & 0x1F)?,
            day: from_bcd(d & 0x3F)?,
            hour: from_bcd(h & 0x3F)?,
            minute: from_bcd(min & 0x7F)?,
            second: from_bcd(s & 0x7F)?,
            weekday: wd & 0x07,
        }
        .to_unix()
    }

    /// Sets the chip (and clears VL); times outside 2000-2099 are not stored.
    pub fn write(&mut self, unix: u32) -> Result<(), Error> {
        let c = Civil::from_unix(unix);
        let Ok(year) = u8::try_from(c.year - 2000) else {
            return Ok(());
        };
        if year > 99 {
            return Ok(());
        }
        // Normal mode, clock running.
        self.i2c.write(ADDR, &[REG_CONTROL1, 0x00])?;
        self.i2c.write(
            ADDR,
            &[
                REG_SECONDS,
                to_bcd(c.second),
                to_bcd(c.minute),
                to_bcd(c.hour),
                to_bcd(c.day),
                c.weekday,
                to_bcd(c.month),
                to_bcd(year),
            ],
        )
    }
}
