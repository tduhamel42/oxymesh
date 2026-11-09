//! The link to the app: companion frames on a serial wire (USB, `<`/`>` framing, ASI:24-73)
//! and over Bluetooth (`crate::ble::Shared`); every frame from the device goes out on both
//! (`docs/spec/companion.md` §1.3).

use esp_hal::Blocking;
use esp_hal::uart::Uart;
use mc_node::{Device, Storage};
use mc_proto::{FRAME_MAX, FROM_DEVICE, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame};
use rand_core::Rng;

use crate::ble::Shared;

/// A serial wire to the computer: the M5's UART0 behind its CH340, the ESP32-S3's own USB on
/// the Heltec.
pub trait Wire {
    fn read_ready(&mut self) -> bool;
    /// Bytes read, 0 on error.
    fn read(&mut self, buf: &mut [u8]) -> usize;
    /// Writes everything, or gives up on an error.
    fn write_all(&mut self, data: &[u8]);
}

impl Wire for Uart<'_, Blocking> {
    fn read_ready(&mut self) -> bool {
        Uart::read_ready(self)
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        Uart::read(self, buf).unwrap_or(0)
    }

    fn write_all(&mut self, data: &[u8]) {
        let mut sent = 0;
        while sent < data.len() {
            match Uart::write(self, data.get(sent..).unwrap_or(&[])) {
                Ok(k) => sent += k,
                Err(_) => break,
            }
        }
    }
}

pub struct AppLink<W: Wire> {
    pub wire: W,
    deframer: UsbDeframer,
    rx_bytes: [u8; 64],
    frame_in: [u8; FRAME_MAX],
    out: [u8; USB_MAX],
}

impl<W: Wire> AppLink<W> {
    pub fn new(wire: W) -> Self {
        Self {
            wire,
            deframer: UsbDeframer::new(TO_DEVICE),
            rx_bytes: [0; 64],
            frame_in: [0; FRAME_MAX],
            out: [0; USB_MAX],
        }
    }

    /// Frames from the app (wire, then Bluetooth) to the device, then the device's frames to
    /// both.
    pub fn poll(
        &mut self,
        dev: &mut Device,
        ble: &Shared,
        now: mc_core::Instant,
        rng: &mut impl Rng,
        st: &mut impl Storage,
    ) {
        while self.wire.read_ready() {
            let n = self.wire.read(&mut self.rx_bytes);
            if n == 0 {
                break;
            }
            for &b in self.rx_bytes.iter().take(n) {
                if let Some(f) = self.deframer.push(b) {
                    let len = f.len();
                    self.frame_in[..len].copy_from_slice(f);
                    dev.on_app_frame(now, &self.frame_in[..len], rng, st);
                }
            }
        }
        loop {
            let n = ble.link.borrow_mut().recv(&mut self.frame_in);
            let Some(n) = n else { break };
            dev.on_app_frame(now, &self.frame_in[..n], rng, st);
        }
        while let Some(f) = dev.poll_app() {
            if ble.link.borrow_mut().send(&f) {
                ble.outbound.signal(());
            }
            if let Ok(n) = usb_frame(FROM_DEVICE, &f, &mut self.out) {
                self.wire.write_all(&self.out[..n]);
            }
        }
    }
}
