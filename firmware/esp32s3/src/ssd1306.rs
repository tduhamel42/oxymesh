//! SSD1306 / SSD1315 128×64 OLED on I2C (Solomon Systech SSD1306 datasheet rev. 1.1: command
//! table §9-10, charge pump application note), written from the datasheet. The frame is
//! `mc_ui::status::Oled` (rows, MSB first, 1 = lit); the controller takes pages of 8 rows, one
//! byte per column, bit 0 at the top.

use esp_hal::Blocking;
use esp_hal::i2c::master::I2c;
use mc_ui::status::{HEIGHT, Oled, WIDTH};

/// I2C address with SA0 low.
const ADDR: u8 = 0x3C;
/// Control bytes: a command stream, a data stream.
const CMD: u8 = 0x00;
const DATA: u8 = 0x40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error;

pub struct Ssd1306<'d> {
    i2c: I2c<'d, Blocking>,
}

impl<'d> Ssd1306<'d> {
    pub fn new(i2c: I2c<'d, Blocking>) -> Self {
        Self { i2c }
    }

    fn cmds(&mut self, c: &[u8]) -> Result<(), Error> {
        let mut buf = [0u8; 8];
        let b = buf.get_mut(..1 + c.len()).ok_or(Error)?;
        b[0] = CMD;
        b[1..].copy_from_slice(c);
        self.i2c.write(ADDR, b).map_err(|_| Error)
    }

    /// Power-up sequence: charge pump on, horizontal addressing, rotated so that row 0 is at the
    /// top with the board's pins down, then on.
    pub fn init(&mut self) -> Result<(), Error> {
        self.cmds(&[0xAE])?; // display off
        self.cmds(&[0xD5, 0x80])?; // clock divide, oscillator
        self.cmds(&[0xA8, 0x3F])?; // multiplex: 64 rows
        self.cmds(&[0xD3, 0x00])?; // display offset
        self.cmds(&[0x40])?; // start line 0
        self.cmds(&[0x8D, 0x14])?; // charge pump on
        self.cmds(&[0x20, 0x00])?; // horizontal addressing
        self.cmds(&[0xA1])?; // segment remap: column 127 = SEG0
        self.cmds(&[0xC8])?; // COM scan from COM63
        self.cmds(&[0xDA, 0x12])?; // COM pins: alternative, no remap
        self.cmds(&[0x81, 0xCF])?; // contrast
        self.cmds(&[0xD9, 0xF1])?; // pre-charge
        self.cmds(&[0xDB, 0x40])?; // VCOMH deselect level
        self.cmds(&[0xA4])?; // follow RAM
        self.cmds(&[0xA6])?; // normal (not inverted)
        self.cmds(&[0x2E])?; // no scrolling
        self.cmds(&[0xAF]) // display on
    }

    /// Panel on (showing RAM) or off (sleep, RAM kept).
    pub fn power(&mut self, on: bool) -> Result<(), Error> {
        self.cmds(&[if on { 0xAF } else { 0xAE }])
    }

    /// Sends the whole frame.
    pub fn show(&mut self, f: &Oled) -> Result<(), Error> {
        self.cmds(&[0x21, 0, (WIDTH - 1) as u8])?; // columns
        self.cmds(&[0x22, 0, (HEIGHT / 8 - 1) as u8])?; // pages
        let mut buf = [0u8; 1 + 32];
        buf[0] = DATA;
        for page in 0..HEIGHT / 8 {
            for chunk in 0..WIDTH / 32 {
                for (i, b) in buf[1..].iter_mut().enumerate() {
                    let x = chunk * 32 + i as u32;
                    let mut v = 0u8;
                    for bit in 0..8 {
                        if f.lit(x, page * 8 + bit) {
                            v |= 1 << bit;
                        }
                    }
                    *b = v;
                }
                self.i2c.write(ADDR, &buf).map_err(|_| Error)?;
            }
        }
        Ok(())
    }
}
