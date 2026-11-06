//! Écran e-paper SSD1681, 200×200 noir et blanc (panneau GDEY0154D67 du ThinkNode M5).
//!
//! Écrit d'après la datasheet SSD1681 (Solomon Systech, rév. 1.0), sans rien reprendre de GxEPD2
//! (GPL, ADR-0007). Deux modes : rafraîchissement complet (forme d'onde de l'OTP, ~2 s, efface la
//! rémanence) et partiel (« display mode 2 », ~0,5 s), qui compare la nouvelle image (RAM 0x24) à
//! l'ancienne (RAM 0x26). La broche BUSY reste haute pendant un rafraîchissement : le pilote ne
//! l'attend pas, il expose [`Ssd1681::busy`] pour que la boucle principale continue (radio, BLE).

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use esp_hal::Blocking;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, Output};
use esp_hal::spi::master::Spi;

pub const WIDTH: u32 = 200;
pub const HEIGHT: u32 = 200;
pub const BUF: usize = (WIDTH * HEIGHT / 8) as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Spi,
    /// L'écran n'a pas fini une commande à temps.
    Timeout,
}

/// Image en mémoire : 1 bit par pixel, 1 = blanc (convention du SSD1681).
pub struct Frame {
    pub bits: [u8; BUF],
}

impl Frame {
    pub const fn new() -> Self {
        Self { bits: [0xFF; BUF] }
    }

    pub fn clear(&mut self) {
        self.bits = [0xFF; BUF];
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(WIDTH, HEIGHT)
    }
}

impl DrawTarget for Frame {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            let (Ok(x), Ok(y)) = (u32::try_from(p.x), u32::try_from(p.y)) else {
                continue;
            };
            if x >= WIDTH || y >= HEIGHT {
                continue;
            }
            let i = (y * (WIDTH / 8) + x / 8) as usize;
            let bit = 0x80u8 >> (x % 8);
            if let Some(b) = self.bits.get_mut(i) {
                // BinaryColor::On = encre noire.
                if c.is_on() {
                    *b &= !bit;
                } else {
                    *b |= bit;
                }
            }
        }
        Ok(())
    }
}

pub struct Ssd1681<'d> {
    spi: Spi<'d, Blocking>,
    cs: Output<'d>,
    dc: Output<'d>,
    rst: Output<'d>,
    busy: Input<'d>,
    delay: Delay,
}

impl<'d> Ssd1681<'d> {
    pub fn new(
        spi: Spi<'d, Blocking>,
        cs: Output<'d>,
        dc: Output<'d>,
        rst: Output<'d>,
        busy: Input<'d>,
    ) -> Self {
        Self {
            spi,
            cs,
            dc,
            rst,
            busy,
            delay: Delay::new(),
        }
    }

    /// L'écran travaille encore (BUSY haute).
    pub fn busy(&self) -> bool {
        self.busy.is_high()
    }

    fn wait(&self, ms: u32) -> Result<(), Error> {
        let mut t = 0;
        while self.busy() {
            if t >= ms {
                return Err(Error::Timeout);
            }
            self.delay.delay_millis(1);
            t += 1;
        }
        Ok(())
    }

    fn command(&mut self, cmd: u8, data: &[u8]) -> Result<(), Error> {
        self.cs.set_low();
        self.dc.set_low();
        let r = self.spi.write(&[cmd]).map_err(|_| Error::Spi);
        self.dc.set_high();
        let r = r.and_then(|()| {
            if data.is_empty() {
                Ok(())
            } else {
                self.spi.write(data).map_err(|_| Error::Spi)
            }
        });
        self.cs.set_high();
        r
    }

    /// Réinitialisation et réglages (§8 de la datasheet) : 200 lignes, adresses croissantes,
    /// fenêtre plein écran, capteur de température interne.
    pub fn init(&mut self) -> Result<(), Error> {
        self.rst.set_low();
        self.delay.delay_millis(10);
        self.rst.set_high();
        self.delay.delay_millis(10);
        self.wait(100)?;
        self.command(0x12, &[])?; // SWRESET
        self.wait(100)?;
        self.command(0x01, &[0xC7, 0x00, 0x00])?; // Driver output control : MUX = 199
        self.command(0x11, &[0x03])?; // Data entry mode : X+, Y+
        self.command(0x44, &[0x00, 0x18])?; // RAM X : octets 0 à 24
        self.command(0x45, &[0x00, 0x00, 0xC7, 0x00])?; // RAM Y : lignes 0 à 199
        self.command(0x3C, &[0x05])?; // Border waveform
        self.command(0x18, &[0x80])?; // Capteur de température interne
        Ok(())
    }

    fn write_ram(&mut self, cmd: u8, frame: &Frame) -> Result<(), Error> {
        self.command(0x4E, &[0x00])?;
        self.command(0x4F, &[0x00, 0x00])?;
        self.command(cmd, &frame.bits)
    }

    /// Rafraîchissement complet : l'image va dans les deux RAM (nouvelle et ancienne) pour que le
    /// prochain partiel parte d'elle. Rend la main tout de suite ; BUSY tombe à la fin (~2 s).
    pub fn show_full(&mut self, frame: &Frame) -> Result<(), Error> {
        self.wait(5000)?;
        self.write_ram(0x24, frame)?;
        self.write_ram(0x26, frame)?;
        self.command(0x22, &[0xF7])?; // horloge, température, LUT de l'OTP, mode 1
        self.command(0x20, &[])
    }

    /// Rafraîchissement partiel (mode 2) depuis l'image précédente, puis l'image devient la
    /// référence du prochain partiel.
    pub fn show_partial(&mut self, frame: &Frame, previous: &Frame) -> Result<(), Error> {
        self.wait(5000)?;
        self.command(0x3C, &[0x80])?; // bord inchangé en partiel
        self.write_ram(0x26, previous)?;
        self.write_ram(0x24, frame)?;
        self.command(0x22, &[0xFF])?; // mode 2 : partiel
        self.command(0x20, &[])
    }

    /// Sommeil profond (consommation quasi nulle, l'image reste).
    pub fn sleep(&mut self) -> Result<(), Error> {
        self.wait(5000)?;
        self.command(0x10, &[0x01])
    }
}
