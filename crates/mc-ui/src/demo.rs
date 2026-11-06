//! U0 spike screen: the widgets the real screens will use (header, list with a reversed
//! selection, wrapped paragraph, sparkline, footer hint), to measure RAM and refresh time on the
//! M5 before building on Ratatui (ADR-0008).

use ratatui_core::layout::Rect;
use ratatui_core::style::{Modifier, Style};
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::list::{List, ListState};
use ratatui_widgets::paragraph::{Paragraph, Wrap};
use ratatui_widgets::sparkline::Sparkline;

/// What the demo shows.
#[derive(Debug, Clone, Default)]
pub struct Demo {
    pub selected: usize,
    /// Frames drawn so far (shown, so each refresh changes something).
    pub frame: u32,
    pub battery_percent: u8,
}

const CONVERSATIONS: [(&str, &str); 6] = [
    ("Alice", "On my way — see you at the café at 3?"),
    ("#paris", "Bob: relay on the roof is back up"),
    ("Public", "Zoé: anyone near Montmartre?"),
    ("Bob", "Thanks!"),
    ("Repeater 7", "Status: 4.02 V, 12 neighbours"),
    ("Chloé", "Bonne soirée !"),
];

const SNR: [u64; 24] = [
    8, 9, 11, 10, 12, 13, 12, 9, 7, 8, 10, 12, 14, 13, 12, 11, 10, 12, 13, 12, 11, 12, 13, 12,
];

/// Header, list, 3-row preview, 2-row sparkline, footer, by plain arithmetic: the constraint
/// solver behind `Layout` allocates about 30 KB per draw on the M5 (ADR-0008).
fn rows(a: Rect) -> [Rect; 5] {
    let row = |y: u16, height: u16| Rect { y, height, ..a };
    let bottom = a.y + a.height;
    let foot = row(bottom.saturating_sub(1), 1.min(a.height));
    let spark = row(foot.y.saturating_sub(2).max(a.y), 2.min(foot.y - a.y));
    let preview = row(spark.y.saturating_sub(3).max(a.y), 3.min(spark.y - a.y));
    let head = row(a.y, 1.min(preview.y - a.y));
    let list = row(
        head.y + head.height,
        preview.y.saturating_sub(head.y + head.height),
    );
    [head, list, preview, spark, foot]
}

pub fn draw(f: &mut Frame<'_>, d: &Demo) {
    let [head, list, preview, spark, foot] = rows(f.area());

    let width = usize::from(head.width);
    let left = alloc::format!("14:02  2 new  #{}", d.frame);
    let right = alloc::format!("{}%", d.battery_percent);
    let pad = width.saturating_sub(left.chars().count() + right.chars().count());
    let header = alloc::format!("{left}{:pad$}{right}", "");
    f.render_widget(
        Paragraph::new(header).style(Style::new().add_modifier(Modifier::REVERSED)),
        head,
    );

    let items = CONVERSATIONS.iter().map(|(who, _)| Line::from(*who));
    let mut state = ListState::default().with_selected(Some(d.selected % CONVERSATIONS.len()));
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ "),
        list,
        &mut state,
    );

    let (_, text) = CONVERSATIONS[d.selected % CONVERSATIONS.len()];
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), preview);
    f.render_widget(Sparkline::default().data(SNR.iter().copied()), spark);
    f.render_widget(Paragraph::new("A▸next  hold▸open  B◂back"), foot);
}
