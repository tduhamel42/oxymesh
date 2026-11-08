//! The ESP32-S3's internal temperature sensor, for device telemetry (the C++ firmware sends it
//! too). esp-hal 1.2.2 has a driver for the C3 and C6 only; this reads the documented SENS
//! registers (ESP32-S3 TRM, "Temperature Sensor") and the factory calibration in eFuse
//! (`TEMP_CALIB`, 0.1 °C, sign in bit 8).
//!
//! Only documented fields are touched (power-up and dump-out under software control). The
//! sensor's range DAC sits behind the analog I2C bus, which esp-hal keeps private: it is left
//! at its reset value, taken as the −10 to 80 °C range (offset 0). The reading is checked
//! against the Heltec V4 (an ESP32-S3 running the C++) in `docs/measurements.md`.

use esp_hal::efuse::{self, TEMP_CALIB};
use esp_hal::peripherals::SENS;

/// Conversion for the −10 to 80 °C range, offset 0 (TRM): °C = 0.4386 × raw − 20.52.
const ADC_FACTOR_MILLI: i32 = 4386;
const OFFSET_DECI_MILLI: i32 = 205_200;

pub struct ChipTemp {
    cal_deci: i32,
}

impl ChipTemp {
    /// Powers the sensor up; it needs a few hundred microseconds before the first reading.
    pub fn new() -> Self {
        let sens = SENS::regs();
        sens.sar_peri_clk_gate_conf()
            .modify(|_, w| w.tsens_clk_en().set_bit());
        sens.sar_tsens_ctrl().modify(|_, w| {
            w.sar_tsens_power_up_force()
                .set_bit()
                .sar_tsens_power_up()
                .set_bit()
        });
        let raw: u16 = efuse::read_field_le(TEMP_CALIB);
        let mag = i32::from(raw & 0xFF);
        let cal_deci = if raw & 0x100 != 0 { -mag } else { mag };
        Self { cal_deci }
    }

    /// Tenths of a degree Celsius, or `None` if the sensor does not answer.
    pub fn read(&self) -> Option<i16> {
        let sens = SENS::regs();
        sens.sar_tsens_ctrl()
            .modify(|_, w| w.sar_tsens_dump_out().set_bit());
        let mut ready = false;
        for _ in 0..10_000 {
            if sens.sar_tsens_ctrl().read().sar_tsens_ready().bit_is_set() {
                ready = true;
                break;
            }
        }
        let raw = i32::from(sens.sar_tsens_ctrl().read().sar_tsens_out().bits());
        sens.sar_tsens_ctrl()
            .modify(|_, w| w.sar_tsens_dump_out().clear_bit());
        if !ready {
            return None;
        }
        let deci = (ADC_FACTOR_MILLI * raw - OFFSET_DECI_MILLI) / 1000 - self.cal_deci;
        i16::try_from(deci).ok()
    }
}

impl Default for ChipTemp {
    fn default() -> Self {
        Self::new()
    }
}
