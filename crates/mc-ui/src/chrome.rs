//! What every screen shares: the header (title and status), the footer (what the buttons do
//! now), and the split of the grid between them (`docs/spec/ui.md` §4). Plain arithmetic, no
//! `Layout` (ADR-0008).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::Rect;
use ratatui_core::style::Style;
use ratatui_core::terminal::Frame;
use ratatui_core::text::{Line, Span};
use ratatui_widgets::paragraph::Paragraph;

use crate::app::Ui;
use crate::clock;
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Areas {
    pub header: Rect,
    pub body: Rect,
    pub footer: Rect,
}

/// Row 0, rows 1 to n−2, row n−1.
pub fn areas(a: Rect) -> Areas {
    let header = Rect {
        height: a.height.min(1),
        ..a
    };
    let footer = Rect {
        y: (a.y + a.height).saturating_sub(1).max(a.y + header.height),
        height: a.height.saturating_sub(header.height).min(1),
        ..a
    };
    let body = Rect {
        y: a.y + header.height,
        height: a.height.saturating_sub(header.height + footer.height),
        ..a
    };
    Areas {
        header,
        body,
        footer,
    }
}

/// The current meaning of the buttons; `None` hides a hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footer {
    pub a_click: Option<&'static str>,
    pub a_hold: Option<&'static str>,
    pub b_click: Option<&'static str>,
    pub b_hold: Option<&'static str>,
}

impl Footer {
    /// A list one level down: next, open, back.
    pub const LIST: Footer = Footer {
        a_click: Some("next"),
        a_hold: Some("open"),
        b_click: Some("back"),
        b_hold: None,
    };
    /// A screen with nothing to select.
    pub const BACK: Footer = Footer {
        a_click: None,
        a_hold: None,
        b_click: Some("back"),
        b_hold: None,
    };
}

/// `s` cut to `n` cells, with `…` when cut.
pub fn fit(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return String::from(s);
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    if n > 0 {
        out.push('…');
    }
    out
}

/// `left` and `right` on one line of `width` cells, `left` cut first.
pub fn spread(left: &str, right: &str, width: usize) -> String {
    let r = right.chars().count();
    if r >= width {
        return fit(right, width);
    }
    let l = fit(left, width.saturating_sub(r + 1));
    let pad = width - l.chars().count() - r;
    format!("{l}{:pad$}{right}", "")
}

/// `text` in lines of at most `width` cells, broken at spaces when possible.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut len = 0usize;
        for word in para.split(' ') {
            let wl = word.chars().count();
            if len > 0 && len + 1 + wl > width {
                out.push(core::mem::take(&mut line));
                len = 0;
            }
            if len > 0 {
                line.push(' ');
                len += 1;
            }
            // A word longer than a line is cut.
            for c in word.chars() {
                if len == width {
                    out.push(core::mem::take(&mut line));
                    len = 0;
                }
                line.push(c);
                len += 1;
            }
        }
        out.push(line);
    }
    out
}

/// Status on the right of the header: unread, backlight, time, battery.
pub fn status(ui: &Ui<'_>, time: bool) -> String {
    let c = ui.ctx;
    let mut parts: Vec<String> = Vec::new();
    let unread = c.dev.history().unread();
    if unread > 0 {
        parts.push(format!("{}{unread}", ui.sym.unread));
    }
    if c.light {
        parts.push(String::from(ui.sym.light));
    }
    if c.ble.state == crate::app::BleState::Connected {
        parts.push(String::from("BT"));
    }
    if time {
        parts.push(clock::hhmm(clock::local(c.unix, c.tz_offset_min)));
    }
    if let Some(p) = c.battery_percent {
        parts.push(format!("{p}%{}", if c.charging { "+" } else { "" }));
    }
    parts.join(" ")
}

pub fn header(f: &mut Frame<'_>, area: Rect, title: &str, time: bool, ui: &Ui<'_>) {
    let width = usize::from(area.width);
    let right = status(ui, time);
    let t = fit(title, width.saturating_sub(right.chars().count() + 1));
    let pad = width.saturating_sub(t.chars().count() + right.chars().count());
    let line = Line::from(alloc::vec![
        Span::styled(t, theme::title()),
        Span::styled(format!("{:pad$}{right}", ""), theme::reversed()),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// The hint line, or a toast in its place.
pub fn footer(f: &mut Frame<'_>, area: Rect, hints: &Footer, toast: Option<&str>, ui: &Ui<'_>) {
    let width = usize::from(area.width);
    let text = match toast {
        Some(t) => fit(t, width),
        None => {
            let s = ui.sym;
            let mut left: Vec<String> = Vec::new();
            if let Some(a) = hints.a_click {
                left.push(format!("{}{a}", s.a_click));
            }
            if let Some(a) = hints.a_hold {
                left.push(format!("{}{a}", s.a_hold));
            }
            let right = match (hints.b_click, hints.b_hold) {
                (Some(b), _) => format!("{}{b}", s.b_click),
                (None, Some(b)) => format!("{}{b}", s.b_hold),
                (None, None) => String::new(),
            };
            spread(&left.join(" "), &right, width)
        }
    };
    let style = if toast.is_some() {
        theme::reversed()
    } else {
        Style::new()
    };
    f.render_widget(Paragraph::new(text).style(style), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_split_the_grid() {
        let a = areas(Rect::new(0, 0, 25, 15));
        assert_eq!(a.header, Rect::new(0, 0, 25, 1));
        assert_eq!(a.body, Rect::new(0, 1, 25, 13));
        assert_eq!(a.footer, Rect::new(0, 14, 25, 1));
        let tiny = areas(Rect::new(0, 0, 10, 1));
        assert_eq!(
            (tiny.header.height, tiny.body.height, tiny.footer.height),
            (1, 0, 0)
        );
    }

    #[test]
    fn fitting_text() {
        assert_eq!(fit("Settings", 8), "Settings");
        assert_eq!(fit("Notifications", 8), "Notific…");
        assert_eq!(spread("A▸next", "B◂back", 14), "A▸next  B◂back");
        assert_eq!(
            wrap("hello there my friend", 11),
            ["hello there", "my friend"]
        );
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 4), [""]);
        assert_eq!(spread("a long left part", "right", 12), "a lon… right");
    }
}
