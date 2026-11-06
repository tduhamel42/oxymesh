//! Minimal synchronous SX1262 driver on esp-hal (SX1261/2 datasheet rev. 2.1, §13).
//!
//! Taken from loranet, extended for MeshCore: DC-DC regulator, current limit, PA at +22 dBm,
//! transmission, preamble and header detection (listen before talk).

use esp_hal::Blocking;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, Output};
use esp_hal::spi::master::Spi;
use esp_hal::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The BUSY pin stayed high: the chip is not responding.
    Busy,
    Spi,
    Param,
}

/// Interrupts (IRQ register, §13.3.1).
pub mod irq {
    pub const TX_DONE: u16 = 1 << 0;
    pub const RX_DONE: u16 = 1 << 1;
    pub const PREAMBLE_DETECTED: u16 = 1 << 2;
    pub const HEADER_VALID: u16 = 1 << 4;
    pub const HEADER_ERR: u16 = 1 << 5;
    pub const CRC_ERR: u16 = 1 << 6;
    pub const CAD_DONE: u16 = 1 << 7;
    pub const CAD_DETECTED: u16 = 1 << 8;
    pub const TIMEOUT: u16 = 1 << 9;
}

/// LoRa settings.
#[derive(Debug, Clone, Copy)]
pub struct LoRaConfig {
    pub freq_hz: u32,
    pub sf: u8,
    /// Bandwidth in Hz: 7_812 … 500_000.
    pub bw_hz: u32,
    /// 1 to 4 for 4/5 to 4/8.
    pub cr: u8,
    pub preamble: u16,
    /// Sync word in the SX127x sense (0x12 private, 0x34 public, 0x2B Meshtastic).
    pub sync_word: u8,
}

impl LoRaConfig {
    fn bw_code(&self) -> Option<u8> {
        Some(match self.bw_hz {
            7_812 | 7_800 => 0x00,
            10_416 | 10_400 => 0x08,
            15_625 | 15_600 => 0x01,
            20_833 | 20_800 => 0x09,
            31_250 => 0x02,
            41_666 | 41_700 => 0x0A,
            62_500 => 0x03,
            125_000 => 0x04,
            250_000 => 0x05,
            500_000 => 0x06,
            _ => return None,
        })
    }

    /// Low data rate optimization: mandatory when the symbol exceeds 16 ms.
    fn ldro(&self) -> bool {
        (1u64 << self.sf) * 1_000_000 / u64::from(self.bw_hz) > 16_000
    }
}

/// Received packet.
#[derive(Debug, Clone, Copy)]
pub struct RxInfo {
    pub len: usize,
    /// Packet power, dBm.
    pub rssi_dbm: i16,
    /// SNR in quarter dB.
    pub snr_qdb: i16,
    pub crc_error: bool,
}

pub struct Sx126x<'d> {
    spi: Spi<'d, Blocking>,
    cs: Output<'d>,
    reset: Output<'d>,
    busy: Input<'d>,
    dio1: Input<'d>,
    delay: Delay,
}

impl<'d> Sx126x<'d> {
    pub fn new(
        spi: Spi<'d, Blocking>,
        cs: Output<'d>,
        reset: Output<'d>,
        busy: Input<'d>,
        dio1: Input<'d>,
    ) -> Self {
        Self {
            spi,
            cs,
            reset,
            busy,
            dio1,
            delay: Delay::new(),
        }
    }

    fn wait_busy(&self) -> Result<(), Error> {
        let start = Instant::now();
        while self.busy.is_high() {
            if start.elapsed() > Duration::from_millis(200) {
                return Err(Error::Busy);
            }
        }
        Ok(())
    }

    /// Exchanges a complete command (opcode, parameters, response bytes) in place.
    fn xfer(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        self.wait_busy()?;
        self.cs.set_low();
        let r = self.spi.transfer(buf).map_err(|_| Error::Spi);
        self.cs.set_high();
        r
    }

    fn cmd(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let mut buf = [0u8; 16];
        let b = buf.get_mut(..bytes.len()).ok_or(Error::Param)?;
        b.copy_from_slice(bytes);
        self.xfer(b)
    }

    /// Hardware reset.
    pub fn reset(&mut self) -> Result<(), Error> {
        self.reset.set_low();
        self.delay.delay_micros(500);
        self.reset.set_high();
        self.delay.delay_millis(10);
        self.wait_busy()
    }

    /// Status byte (§13.5.1): chip mode (bits 6-4) and command status (bits 3-1).
    pub fn status(&mut self) -> Result<u8, Error> {
        let mut b = [0xC0, 0x00];
        self.xfer(&mut b)?;
        Ok(b[1])
    }

    pub fn read_reg(&mut self, addr: u16, out: &mut [u8]) -> Result<(), Error> {
        let mut buf = [0u8; 20];
        let n = 4 + out.len();
        let b = buf.get_mut(..n).ok_or(Error::Param)?;
        b[0] = 0x1D;
        b[1] = (addr >> 8) as u8;
        b[2] = addr as u8;
        self.xfer(b)?;
        out.copy_from_slice(&b[4..n]);
        Ok(())
    }

    pub fn write_reg(&mut self, addr: u16, data: &[u8]) -> Result<(), Error> {
        let mut buf = [0u8; 16];
        let n = 3 + data.len();
        let b = buf.get_mut(..n).ok_or(Error::Param)?;
        b[0] = 0x0D;
        b[1] = (addr >> 8) as u8;
        b[2] = addr as u8;
        b[3..n].copy_from_slice(data);
        self.xfer(b)
    }

    /// Internal errors (§13.6.1): bit 5 = oscillator start, bits 0-6 = calibrations.
    pub fn device_errors(&mut self) -> Result<u16, Error> {
        let mut b = [0x17, 0, 0, 0];
        self.xfer(&mut b)?;
        Ok(u16::from_be_bytes([b[2], b[3]]))
    }

    pub fn clear_device_errors(&mut self) -> Result<(), Error> {
        self.cmd(&[0x07, 0x00, 0x00])
    }

    /// Common initialization: standby, TCXO on DIO3, calibration, regulator, RF switch on DIO2.
    pub fn init(&mut self, tcxo_millivolts: u16, dcdc: bool) -> Result<(), Error> {
        self.reset()?;
        self.cmd(&[0x80, 0x00])?; // SetStandby(STDBY_RC)
        let v = match tcxo_millivolts {
            1600 => 0x00,
            1700 => 0x01,
            1800 => 0x02,
            2200 => 0x03,
            2400 => 0x04,
            2700 => 0x05,
            3000 => 0x06,
            3300 => 0x07,
            _ => return Err(Error::Param),
        };
        // TCXO startup delay: 5 ms in 15.625 µs steps = 320.
        self.cmd(&[0x97, v, 0x00, 0x01, 0x40])?; // SetDIO3AsTcxoCtrl
        self.cmd(&[0x89, 0x7F])?; // Calibrate(all blocks)
        self.delay.delay_millis(5);
        self.wait_busy()?;
        self.clear_device_errors()?;
        self.cmd(&[0x96, u8::from(dcdc)])?; // SetRegulatorMode(LDO or DC-DC)
        self.cmd(&[0x9D, 0x01])?; // SetDIO2AsRfSwitchCtrl
        self.cmd(&[0x8A, 0x01])?; // SetPacketType(LoRa)
        Ok(())
    }

    /// Sets frequency, modulation, sync word and gain; the chip stays in standby.
    pub fn configure(&mut self, c: &LoRaConfig) -> Result<(), Error> {
        let bw = c.bw_code().ok_or(Error::Param)?;
        if !(5..=12).contains(&c.sf) || !(1..=4).contains(&c.cr) {
            return Err(Error::Param);
        }
        self.cmd(&[0x80, 0x00])?;
        // Image calibration for 863-870 MHz (§9.2.1).
        self.cmd(&[0x98, 0xD7, 0xDB])?;
        let f = ((u64::from(c.freq_hz) << 25) / 32_000_000) as u32;
        let fb = f.to_be_bytes();
        self.cmd(&[0x86, fb[0], fb[1], fb[2], fb[3]])?; // SetRfFrequency
        self.cmd(&[0x8F, 0x00, 0x00])?; // SetBufferBaseAddress
        self.cmd(&[0x8B, c.sf, bw, c.cr, u8::from(c.ldro())])?; // SetModulationParams
        self.packet_params(c.preamble, 0xFF)?;
        // Errata fix (§15.4): with standard IQ, bit 2 of register 0x0736 must be 1.
        let mut iq = [0u8];
        self.read_reg(0x0736, &mut iq)?;
        self.write_reg(0x0736, &[iq[0] | 0x04])?;
        let s = c.sync_word;
        self.write_reg(0x0740, &[(s & 0xF0) | 0x04, ((s & 0x0F) << 4) | 0x04])?;
        self.write_reg(0x08AC, &[0x96]) // boosted RX gain
    }

    /// SetPacketParams: preamble, explicit header, length, CRC, standard IQ.
    fn packet_params(&mut self, preamble: u16, len: u8) -> Result<(), Error> {
        let p = preamble.to_be_bytes();
        self.cmd(&[0x8C, p[0], p[1], 0x00, len, 0x01, 0x00])
    }

    /// SX1262 TX power (−9 to +22 dBm), current limit `ocp_ma`.
    /// High-power PA: `SetPaConfig(0x04, 0x07, 0x00, 0x01)` (§13.1.14), 200 µs ramp, and the PA
    /// clamping fix (errata §15.2, as in RadioLib).
    pub fn set_tx_power(&mut self, dbm: i8, ocp_ma: u16) -> Result<(), Error> {
        let dbm = dbm.clamp(-9, 22);
        self.cmd(&[0x95, 0x04, 0x07, 0x00, 0x01])?;
        self.cmd(&[0x8E, dbm as u8, 0x04])?;
        let ocp = u8::try_from(u32::from(ocp_ma) * 10 / 25).unwrap_or(0x38);
        self.write_reg(0x08E7, &[ocp])?;
        let mut clamp = [0u8];
        self.read_reg(0x08D8, &mut clamp)?;
        self.write_reg(0x08D8, &[clamp[0] | 0x1E])
    }

    /// Continuous reception. DIO1 signals end of reception and errors; preamble and header are
    /// only read from the IRQ register, for listen before talk.
    pub fn start_rx(&mut self, preamble: u16) -> Result<(), Error> {
        self.cmd(&[0x80, 0x00])?;
        self.packet_params(preamble, 0xFF)?;
        let mask = irq::RX_DONE
            | irq::CRC_ERR
            | irq::HEADER_ERR
            | irq::TIMEOUT
            | irq::PREAMBLE_DETECTED
            | irq::HEADER_VALID;
        let dio1 = irq::RX_DONE | irq::CRC_ERR | irq::HEADER_ERR | irq::TIMEOUT;
        let m = mask.to_be_bytes();
        let d = dio1.to_be_bytes();
        self.cmd(&[0x08, m[0], m[1], d[0], d[1], 0, 0, 0, 0])?; // SetDioIrqParams
        self.clear_irq()?;
        self.cmd(&[0x82, 0xFF, 0xFF, 0xFF]) // SetRx(continuous)
    }

    /// Transmits `data` (at most 255 bytes). End of transmission is signalled on DIO1 (TX_DONE).
    pub fn transmit(&mut self, preamble: u16, data: &[u8]) -> Result<(), Error> {
        let len = u8::try_from(data.len()).map_err(|_| Error::Param)?;
        self.cmd(&[0x80, 0x00])?;
        self.cmd(&[0x8F, 0x00, 0x00])?;
        let mut buf = [0u8; 257];
        let b = buf.get_mut(..2 + data.len()).ok_or(Error::Param)?;
        b[0] = 0x0E; // WriteBuffer
        b[1] = 0x00;
        b[2..].copy_from_slice(data);
        self.xfer(b)?;
        self.packet_params(preamble, len)?;
        let m = (irq::TX_DONE | irq::TIMEOUT).to_be_bytes();
        self.cmd(&[0x08, m[0], m[1], m[0], m[1], 0, 0, 0, 0])?;
        self.clear_irq()?;
        self.cmd(&[0x83, 0x00, 0x00, 0x00]) // SetTx without timeout
    }

    /// Sleep, cold start (configuration lost): for hibernation, the boot reconfigures.
    pub fn sleep(&mut self) -> Result<(), Error> {
        self.cmd(&[0x80, 0x00])?;
        self.cmd(&[0x84, 0x00])
    }

    pub fn dio1(&self) -> bool {
        self.dio1.is_high()
    }

    pub fn irq_status(&mut self) -> Result<u16, Error> {
        let mut b = [0x12, 0, 0, 0];
        self.xfer(&mut b)?;
        Ok(u16::from_be_bytes([b[2], b[3]]))
    }

    pub fn clear_irq(&mut self) -> Result<(), Error> {
        self.cmd(&[0x02, 0xFF, 0xFF])
    }

    /// Reads the last received packet into `out` (up to its size).
    pub fn read_packet(&mut self, out: &mut [u8], crc_error: bool) -> Result<RxInfo, Error> {
        let mut st = [0x13, 0, 0, 0];
        self.xfer(&mut st)?;
        let (len, start) = (usize::from(st[2]), st[3]);
        let n = len.min(out.len());
        let mut buf = [0u8; 258];
        let b = buf.get_mut(..3 + n).ok_or(Error::Param)?;
        b[0] = 0x1E;
        b[1] = start;
        self.xfer(b)?;
        out[..n].copy_from_slice(&b[3..3 + n]);
        let mut ps = [0x14, 0, 0, 0, 0];
        self.xfer(&mut ps)?;
        Ok(RxInfo {
            len,
            rssi_dbm: -(i16::from(ps[2]) / 2),
            snr_qdb: i16::from(ps[3] as i8),
            crc_error,
        })
    }

    /// Instantaneous channel power, dBm.
    pub fn rssi_inst(&mut self) -> Result<i16, Error> {
        let mut b = [0x15, 0, 0];
        self.xfer(&mut b)?;
        Ok(-(i16::from(b[2]) / 2))
    }
}
