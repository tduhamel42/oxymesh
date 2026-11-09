//! Heltec WiFi LoRa 32 V4 (`docs/spec/heltec-v4.md`): ESP32-S3R2 (16 MB flash, 2 MB PSRAM),
//! SX1262 behind a front-end amplifier, 128×64 OLED, one button, native USB. Pins and front end
//! as MeshCore drives them (`variants/heltec_v4/`, MIT), cross-checked with Meshtastic's
//! variant (hardware facts only) and loranet (same author).

use esp_hal::Blocking;
use esp_hal::gpio::{Flex, InputConfig, Level, Output, OutputConfig, OutputPin, Pin, Pull};
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use mc_node::Prefs;
use mc_node::device::FrontEnd as Kind;

use crate::run::link::Wire;
use crate::run::radio::FrontEnd;

/// TCXO on DIO3.
pub const TCXO_MILLIVOLTS: u16 = 1800;
/// PA current limit (`SX126X_CURRENT_LIMIT`).
pub const OCP_MA: u16 = 140;
/// The SX1262 setting's maximum (`MAX_LORA_TX_POWER`); the amplifier adds 7 to 11 dB.
pub const TX_POWER_MAX: i8 = 22;
/// Battery divider × ADC reference correction (Meshtastic 4.9 × 1.045, MeshCore 5.42): to be
/// calibrated.
pub const BATTERY_MULT_MILLI: u32 = 5120;

fn wait_ms(ms: u64) {
    let t = esp_hal::time::Instant::now();
    while t.elapsed() < esp_hal::time::Duration::from_millis(ms) {}
}

/// The front end (`LoRaFEMControl`, `variants/heltec_v4/LoRaFEMControl.cpp`): GC1109 (V4.2)
/// or KCT8103L (V4.3), told apart at boot by GPIO2's pull level (the KCT8103L pulls its CSD up,
/// the GC1109 its enable down). GPIO7 powers it; DIO2 switches its TX/RX path.
pub struct Fem<'d> {
    _power: Output<'d>,
    /// GPIO2: GC1109 enable / KCT8103L CSD (high = on).
    enable: Flex<'d>,
    /// GC1109 CPS on GPIO46 (high = full PA) or KCT8103L CTX on GPIO5 (high = LNA bypassed).
    mode: Output<'d>,
    pub kind: Kind,
    lna: bool,
}

impl<'d> Fem<'d> {
    /// Powers the front end and finds which one it is.
    pub fn new(
        power: impl OutputPin + 'd,
        gpio2: impl Pin + 'd,
        gpio5: impl OutputPin + 'd,
        gpio46: impl OutputPin + 'd,
        lna: bool,
    ) -> Self {
        let out = OutputConfig::default();
        let power = Output::new(power, Level::High, out);
        wait_ms(1);
        let mut enable = Flex::new(gpio2);
        enable.apply_input_config(&InputConfig::default().with_pull(Pull::None));
        enable.set_input_enable(true);
        wait_ms(1);
        let kct = enable.is_high();
        enable.apply_output_config(&out);
        enable.set_high();
        enable.set_output_enable(true);
        let (kind, mode) = if kct {
            (Kind::PaLna, Output::new(gpio5, Level::High, out))
        } else {
            (Kind::Pa, Output::new(gpio46, Level::Low, out))
        };
        let mut f = Self {
            _power: power,
            enable,
            mode,
            kind,
            lna,
        };
        f.rx();
        f
    }

    /// The name the C++ reports (`HeltecV4Board::getManufacturerName`).
    pub fn board_name(&self) -> &'static str {
        if self.kind == Kind::PaLna {
            "Heltec V4.3 OLED"
        } else {
            "Heltec V4 OLED"
        }
    }
}

impl FrontEnd for Fem<'_> {
    fn tx(&mut self) {
        self.enable.set_high();
        // GC1109: full PA; KCT8103L: CTX high in transmit.
        self.mode.set_high();
    }

    fn rx(&mut self) {
        self.enable.set_high();
        match self.kind {
            Kind::PaLna => self
                .mode
                .set_level(if self.lna { Level::Low } else { Level::High }),
            _ => self.mode.set_low(),
        }
    }

    fn sleep(&mut self) {
        self.enable.set_low();
        if self.kind != Kind::PaLna {
            self.mode.set_low();
        }
    }

    fn apply(&mut self, prefs: &Prefs) {
        self.lna = prefs.fem_rxgain;
    }
}

/// The ESP32-S3's own USB (USB-Serial-JTAG) as the app's serial wire. Writes never wait for a
/// computer that is not reading: a frame nobody takes within a few milliseconds is dropped,
/// so the node runs on unplugged.
pub struct UsbWire<'d> {
    pub usb: UsbSerialJtag<'d, Blocking>,
}

impl Wire for UsbWire<'_> {
    fn read_ready(&mut self) -> bool {
        true
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        let mut n = 0;
        for b in buf.iter_mut() {
            match self.usb.read_byte() {
                Ok(v) => {
                    *b = v;
                    n += 1;
                }
                Err(_) => break,
            }
        }
        n
    }

    fn write_all(&mut self, data: &[u8]) {
        let start = esp_hal::time::Instant::now();
        for &b in data {
            loop {
                match self.usb.write_byte_nb(b) {
                    Ok(()) => break,
                    Err(_) => {
                        let _ = self.usb.flush_tx_nb();
                        if start.elapsed() > esp_hal::time::Duration::from_millis(5) {
                            return;
                        }
                    }
                }
            }
        }
        let _ = self.usb.flush_tx_nb();
    }
}
