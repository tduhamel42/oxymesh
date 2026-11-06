//! The 200×200 one-bit image sent to the SSD1681 panel, and the text grids that fit on it.
//!
//! The firmware sends `Frame::bits` to the panel as is; the emulator and tests decode the same
//! bytes, so what they show is exactly what the panel gets.

use embedded_graphics::Pixel;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, OriginDimensions, Size};

pub const WIDTH: u32 = 200;
pub const HEIGHT: u32 = 200;
/// Bytes in a frame: one bit per pixel, rows of 25 bytes.
pub const BUF: usize = (WIDTH * HEIGHT / 8) as usize;

/// A text grid: columns × rows of character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
}

/// 8×13 font: 25 columns × 15 rows (5 spare pixel rows).
pub const LARGE: Grid = Grid { cols: 25, rows: 15 };
/// 6×10 font: 33 columns × 20 rows (2 spare pixel columns).
pub const COMPACT: Grid = Grid { cols: 33, rows: 20 };

/// In-memory image: 1 bit per pixel, MSB first, 1 = white (SSD1681 convention).
/// `BinaryColor::On` is ink.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub bits: [u8; BUF],
}

impl core::fmt::Debug for Frame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let ink = self.bits.iter().map(|b| b.count_zeros()).sum::<u32>();
        write!(f, "Frame({ink} ink pixels)")
    }
}

impl Frame {
    pub const fn new() -> Self {
        Self { bits: [0xFF; BUF] }
    }

    pub fn clear(&mut self) {
        self.bits = [0xFF; BUF];
    }

    /// Is the pixel at (x, y) ink? Outside the panel: no.
    pub fn ink(&self, x: u32, y: u32) -> bool {
        if x >= WIDTH || y >= HEIGHT {
            return false;
        }
        let i = (y * (WIDTH / 8) + x / 8) as usize;
        self.bits
            .get(i)
            .is_some_and(|b| b & (0x80u8 >> (x % 8)) == 0)
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

/// The mousefood backend drawing `grid`-sized cells straight into `frame` (no framebuffer copy:
/// the frame is the framebuffer).
pub fn backend(
    frame: &mut Frame,
    grid: Grid,
) -> mousefood::EmbeddedBackend<'_, Frame, BinaryColor> {
    let (regular, bold) = if grid == COMPACT {
        (mousefood::fonts::MONO_6X10, None)
    } else {
        (
            mousefood::fonts::MONO_8X13,
            Some(mousefood::fonts::MONO_8X13_BOLD),
        )
    };
    // Every field spelled out: `EmbeddedBackendConfig::default()` builds the default font atlas
    // and leaks it (about 3.8 KB on the M5 per call).
    mousefood::EmbeddedBackend::new(
        frame,
        mousefood::EmbeddedBackendConfig {
            flush_callback: alloc::boxed::Box::new(|_| {}),
            font_regular: regular,
            font_bold: bold,
            font_italic: None,
            vertical_alignment: mousefood::TerminalAlignment::Start,
            horizontal_alignment: mousefood::TerminalAlignment::Start,
            color_theme: mousefood::ColorTheme::default(),
            cursor: mousefood::CursorConfig::default(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::prelude::*;
    use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

    #[test]
    fn pixels_land_in_the_panel_layout() {
        let mut f = Frame::new();
        let _ = Rectangle::new(Point::new(8, 1), Size::new(1, 1))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut f);
        // Row 1 starts at byte 25; x = 8 is the MSB of its second byte.
        assert_eq!(f.bits[26], 0x7F);
        assert!(f.ink(8, 1));
        assert!(!f.ink(9, 1));
        assert!(!f.ink(500, 1));
    }
}
