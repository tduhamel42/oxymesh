//! Pixel overlays drawn over the Ratatui frame on the panel: big clock digits and QR codes
//! (`docs/spec/ui.md` §1).
//!
//! The screen draws a text fallback in the same cells (what the terminal emulator shows) and,
//! on the panel, marks those cells `CellDiffOption::Skip` so Ratatui leaves the pixels alone.
//! An overlay always repaints its whole rectangle.

use alloc::string::String;
use alloc::vec;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};

/// A rectangle in panel pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl PixelRect {
    fn rect(&self) -> Rectangle {
        Rectangle::new(Point::new(self.x, self.y), Size::new(self.w, self.h))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// `HH:MM` in seven-segment digits as tall as the rectangle allows; `None` draws dashes.
    Clock {
        at: PixelRect,
        time: Option<(u8, u8)>,
    },
    /// A QR code of `text`, as large as fits, centered.
    Qr { at: PixelRect, text: String },
    /// A six-digit PIN in seven-segment digits, two groups of three, as tall as fits.
    Pin { at: PixelRect, pin: u32 },
    /// A map: us at the center (a cross), nodes as rings with their names; positions in
    /// thousandths of the half-width, east and north.
    Map {
        at: PixelRect,
        points: alloc::vec::Vec<(String, i16, i16)>,
    },
}

impl Overlay {
    pub fn area(&self) -> PixelRect {
        match self {
            Overlay::Clock { at, .. }
            | Overlay::Pin { at, .. }
            | Overlay::Qr { at, .. }
            | Overlay::Map { at, .. } => *at,
        }
    }
}

/// Largest QR version we encode (57×57 modules; a contact link needs 6 or 7).
const QR_MAX: Version = Version::new(10);

pub fn draw<D: DrawTarget<Color = BinaryColor>>(o: &Overlay, d: &mut D) -> Result<(), D::Error> {
    let at = o.area();
    at.rect()
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
        .draw(d)?;
    match o {
        Overlay::Clock { time, .. } => clock(at, *time, d),
        Overlay::Pin { pin, .. } => pin_digits(at, *pin, d),
        Overlay::Qr { text, .. } => qr(at, text, d),
        Overlay::Map { points, .. } => map(at, points, d),
    }
}

fn map<D: DrawTarget<Color = BinaryColor>>(
    at: PixelRect,
    points: &[(String, i16, i16)],
    d: &mut D,
) -> Result<(), D::Error> {
    use embedded_graphics::mono_font::MonoTextStyle;
    use embedded_graphics::primitives::{Circle, Line};
    use embedded_graphics::text::{Baseline, Text};
    let ink = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    let cx = at.x + at.w as i32 / 2;
    let cy = at.y + at.h as i32 / 2;
    let half = (at.w.min(at.h) as i32 / 2 - 4).max(8);
    let font = MonoTextStyle::new(&mousefood::fonts::MONO_6X10, BinaryColor::On);
    // Us: a cross in a ring.
    Line::new(Point::new(cx - 7, cy), Point::new(cx + 7, cy))
        .into_styled(ink)
        .draw(d)?;
    Line::new(Point::new(cx, cy - 7), Point::new(cx, cy + 7))
        .into_styled(ink)
        .draw(d)?;
    Circle::with_center(Point::new(cx, cy), 9)
        .into_styled(ink)
        .draw(d)?;
    for (label, e, n) in points {
        let x = cx + i32::from(*e) * half / 1000;
        let y = cy - i32::from(*n) * half / 1000;
        if x < at.x || y < at.y || x >= at.x + at.w as i32 || y >= at.y + at.h as i32 {
            continue;
        }
        Circle::with_center(Point::new(x, y), 9)
            .into_styled(ink)
            .draw(d)?;
        Circle::with_center(Point::new(x, y), 3)
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(d)?;
        // Name to the right, or to the left near the right edge.
        let w = label.chars().count() as i32 * 6;
        let tx = if x + 7 + w > at.x + at.w as i32 {
            x - 7 - w
        } else {
            x + 7
        };
        Text::with_baseline(label, Point::new(tx, y - 5), font, Baseline::Top).draw(d)?;
    }
    Ok(())
}

fn fill<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<(), D::Error> {
    if w <= 0 || h <= 0 {
        return Ok(());
    }
    Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(d)
}

/// Segments a to g, bit 0 = a.
const DIGITS: [u8; 10] = [
    0b011_1111, 0b000_0110, 0b101_1011, 0b100_1111, 0b110_0110, 0b110_1101, 0b111_1101, 0b000_0111,
    0b111_1111, 0b110_1111,
];
const DASH: u8 = 0b100_0000;

fn clock<D: DrawTarget<Color = BinaryColor>>(
    at: PixelRect,
    time: Option<(u8, u8)>,
    d: &mut D,
) -> Result<(), D::Error> {
    // Digit height from the rectangle, width 6/11 of it, stroke 1/8; colon one stroke wide.
    let h = (at.h as i32 - 4).max(10);
    let mut w = h * 6 / 11;
    let mut t = (h / 8).max(2);
    let gap = (t * 2).max(4);
    let total = |w: i32, t: i32| 4 * w + 3 * gap + t + gap;
    while total(w, t) > at.w as i32 && w > 4 {
        w -= 1;
        t = (w / 5).clamp(2, t);
    }
    let x0 = at.x + (at.w as i32 - total(w, t)) / 2;
    let y0 = at.y + (at.h as i32 - h) / 2;
    let segs = match time {
        Some((hh, mm)) => [
            DIGITS[usize::from(hh / 10 % 10)],
            DIGITS[usize::from(hh % 10)],
            DIGITS[usize::from(mm / 10 % 10)],
            DIGITS[usize::from(mm % 10)],
        ],
        None => [DASH; 4],
    };
    let mut x = x0;
    for (i, s) in segs.iter().enumerate() {
        if i == 2 {
            // Colon.
            fill(d, x, y0 + h / 3 - t / 2, t, t)?;
            fill(d, x, y0 + 2 * h / 3 - t / 2, t, t)?;
            x += t + gap;
        }
        digit(d, x, y0, w, h, t, *s)?;
        x += w + gap;
    }
    Ok(())
}

fn pin_digits<D: DrawTarget<Color = BinaryColor>>(
    at: PixelRect,
    pin: u32,
    d: &mut D,
) -> Result<(), D::Error> {
    // Like the clock: height from the rectangle, then narrower until six digits and the
    // middle gap fit.
    let h = (at.h as i32 - 4).max(10);
    let mut w = h * 6 / 11;
    let mut t = (h / 8).max(2);
    let gap = (t * 2).max(4);
    let total = |w: i32| 6 * w + 5 * gap + 2 * gap;
    while total(w) > at.w as i32 && w > 4 {
        w -= 1;
        t = (w / 5).clamp(2, t);
    }
    let mut x = at.x + (at.w as i32 - total(w)) / 2;
    let y = at.y + (at.h as i32 - h) / 2;
    let mut p = pin % 1_000_000;
    let mut digits = [0u8; 6];
    for slot in digits.iter_mut().rev() {
        *slot = (p % 10) as u8;
        p /= 10;
    }
    for (i, v) in digits.iter().enumerate() {
        if i == 3 {
            x += 2 * gap;
        }
        digit(d, x, y, w, h, t, DIGITS[usize::from(*v)])?;
        x += w + gap;
    }
    Ok(())
}

fn digit<D: DrawTarget<Color = BinaryColor>>(
    d: &mut D,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    t: i32,
    segs: u8,
) -> Result<(), D::Error> {
    let mid = y + h / 2;
    let on = |bit: u8| segs & (1 << bit) != 0;
    // Horizontal bars leave the corners to the vertical ones: classic segment look.
    if on(0) {
        fill(d, x + t, y, w - 2 * t, t)?;
    }
    if on(6) {
        fill(d, x + t, mid - t / 2, w - 2 * t, t)?;
    }
    if on(3) {
        fill(d, x + t, y + h - t, w - 2 * t, t)?;
    }
    let upper = (y + t, mid - t / 2 - (y + t));
    let lower = (mid + (t + 1) / 2, y + h - t - (mid + (t + 1) / 2));
    if on(5) {
        fill(d, x, upper.0, t, upper.1)?;
    }
    if on(1) {
        fill(d, x + w - t, upper.0, t, upper.1)?;
    }
    if on(4) {
        fill(d, x, lower.0, t, lower.1)?;
    }
    if on(2) {
        fill(d, x + w - t, lower.0, t, lower.1)?;
    }
    Ok(())
}

fn qr<D: DrawTarget<Color = BinaryColor>>(
    at: PixelRect,
    text: &str,
    d: &mut D,
) -> Result<(), D::Error> {
    let mut tmp = vec![0u8; QR_MAX.buffer_len()];
    let mut out = vec![0u8; QR_MAX.buffer_len()];
    let Ok(code) = QrCode::encode_text(
        text,
        &mut tmp,
        &mut out,
        QrCodeEcc::Medium,
        Version::MIN,
        QR_MAX,
        None,
        true,
    ) else {
        return Ok(());
    };
    // Two modules of quiet zone on each side; the panel around is white anyway.
    let n = code.size();
    let side = at.w.min(at.h) as i32;
    let m = (side / (n + 4)).max(1);
    let x0 = at.x + (at.w as i32 - n * m) / 2;
    let y0 = at.y + (at.h as i32 - n * m) / 2;
    for y in 0..n {
        for x in 0..n {
            if code.get_module(x, y) {
                fill(d, x0 + x * m, y0 + y * m, m, m)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::Frame;

    #[test]
    fn clock_draws_inside_its_rectangle() {
        let mut f = Frame::new();
        let at = PixelRect {
            x: 0,
            y: 13,
            w: 200,
            h: 52,
        };
        draw(
            &Overlay::Clock {
                at,
                time: Some((18, 8)),
            },
            &mut f,
        )
        .unwrap();
        let inside = |x: u32, y: u32| (13..65).contains(&y) && x < 200;
        let mut ink = 0;
        for y in 0..200 {
            for x in 0..200 {
                if f.ink(x, y) {
                    assert!(inside(x, y), "ink outside at {x},{y}");
                    ink += 1;
                }
            }
        }
        assert!(ink > 800, "{ink}");
    }

    #[test]
    fn contact_qr_fits_the_body() {
        let mut f = Frame::new();
        let text = "meshcore://contact/add?name=Morgan&public_key=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef&type=1";
        let at = PixelRect {
            x: 0,
            y: 13,
            w: 200,
            h: 156,
        };
        draw(
            &Overlay::Qr {
                at,
                text: text.into(),
            },
            &mut f,
        )
        .unwrap();
        // Finder pattern: the top-left module row starts with 7 dark modules.
        let ink = f.bits.iter().map(|b| b.count_zeros()).sum::<u32>();
        assert!(ink > 5000, "{ink}");
    }
}
