//! The monochrome look: styles, and the symbols each grid's font can draw
//! (`docs/spec/ui.md` §2).
//!
//! The bold 8×13 font has no arrows, dots or sun: only plain title text is ever bold.

use ratatui_core::style::{Modifier, Style};

use crate::panel::{COMPACT, Grid};

/// Symbols for one grid. Every character here exists in that grid's regular font (tested).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symbols {
    /// Footer, A click: `A▸next`.
    pub a_click: &'static str,
    /// Footer, A long press: `hold▸open` (the 8×13 ⏎ glyph reads as a music note).
    pub a_hold: &'static str,
    /// Footer, B click: `B◂back`.
    pub b_click: &'static str,
    /// Footer, B long press when B has no click meaning: `B☀`.
    pub b_hold: &'static str,
    /// Marker of the selected row (two cells).
    pub selected: &'static str,
    /// Unread count in the header.
    pub unread: &'static str,
    /// Unread count of a muted channel: there, but not calling.
    pub quiet: &'static str,
    /// Backlight on, in the header.
    pub light: &'static str,
    /// Separator inside a line.
    pub dot: &'static str,
    /// "Previous / next item" hints inside a row.
    pub left: &'static str,
    pub right: &'static str,
}

pub const LARGE_SYMBOLS: Symbols = Symbols {
    a_click: "A▸",
    a_hold: "hold▸",
    b_click: "B◂",
    b_hold: "B",
    selected: "▸ ",
    unread: "●",
    quiet: "○",
    light: "☀",
    dot: "·",
    left: "◂",
    right: "▸",
};

/// The 6×10 font has no ▸ ◂: filled triangles instead.
pub const COMPACT_SYMBOLS: Symbols = Symbols {
    a_click: "A►",
    a_hold: "hold►",
    b_click: "B◄",
    b_hold: "B",
    selected: "► ",
    unread: "●",
    quiet: "○",
    light: "☀",
    dot: "·",
    left: "◄",
    right: "►",
};

pub fn symbols(grid: Grid) -> &'static Symbols {
    if grid == COMPACT {
        &COMPACT_SYMBOLS
    } else {
        &LARGE_SYMBOLS
    }
}

/// Header bar and selected rows.
pub fn reversed() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Screen titles (letters only: the bold font has no symbols).
pub fn title() -> Style {
    Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
}

/// Section labels inside a screen.
pub fn label() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::mono_font::MonoFont;
    use mousefood::fonts::{MONO_6X10, MONO_8X13};

    fn missing(font: &MonoFont<'_>, s: &Symbols) -> std::string::String {
        let unknown = font.glyph_mapping.index('\u{10FFFD}');
        [
            s.a_click, s.a_hold, s.b_click, s.b_hold, s.selected, s.unread, s.quiet, s.light,
            s.dot, s.left, s.right,
        ]
        .iter()
        .flat_map(|t| t.chars())
        .filter(|&c| font.glyph_mapping.index(c) == unknown)
        .collect()
    }

    #[test]
    fn every_symbol_exists_in_its_font() {
        assert_eq!(missing(&MONO_8X13, &LARGE_SYMBOLS), "");
        assert_eq!(missing(&MONO_6X10, &COMPACT_SYMBOLS), "");
    }
}
