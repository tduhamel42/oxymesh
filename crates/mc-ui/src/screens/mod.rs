//! The screens (`docs/spec/ui.md` §6) and what they share: list drawing and cursor moves.

mod home;
mod info;
mod lists;
mod menu;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::list::{List, ListState};

use crate::app::{Input, Screen, Ui};
use crate::chrome;
use crate::theme;

pub use home::Home;
pub use info::{About, Card, Messages, Soon, contact_link as info_link};
pub use lists::{Channels, Contacts};
pub use menu::{Action, Item, Menu};

/// Where a menu item leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    Messages,
    Contacts,
    Channels,
    Tools,
    Settings,
    Identity,
    System,
    About,
    MyCard,
}

pub fn open(d: Dest) -> Box<dyn Screen> {
    match d {
        Dest::Messages => Box::new(Messages),
        Dest::Contacts => Box::new(Contacts::default()),
        Dest::Channels => Box::new(Channels::default()),
        Dest::Tools => Box::new(menu::tools()),
        Dest::Settings => Box::new(menu::settings()),
        Dest::Identity => Box::new(menu::identity()),
        Dest::System => Box::new(menu::system()),
        Dest::About => Box::new(About),
        Dest::MyCard => Box::new(Card),
    }
}

/// Moves the cursor of a list of `len` rows; true if the selected row is to be opened.
pub(crate) fn step(state: &mut ListState, len: usize, input: Input) -> bool {
    if len == 0 {
        return false;
    }
    let sel = state.selected().unwrap_or(0).min(len - 1);
    match input {
        Input::Next(n) => state.select(Some((sel + usize::from(n)) % len)),
        Input::Prev => state.select(Some((sel + len - 1) % len)),
        Input::Open => {
            state.select(Some(sel));
            return true;
        }
    }
    false
}

/// Rows of `label … detail`, the selected one reversed and marked.
pub(crate) fn draw_list(
    f: &mut Frame<'_>,
    area: Rect,
    rows: &[(String, String)],
    state: &mut ListState,
    ui: &Ui<'_>,
) {
    if state.selected().is_none() && !rows.is_empty() {
        state.select(Some(0));
    }
    let marker = ui.sym.selected;
    let width = usize::from(area.width).saturating_sub(marker.chars().count());
    let items: Vec<Line<'_>> = rows
        .iter()
        .map(|(l, d)| Line::from(chrome::spread(l, d, width)))
        .collect();
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(theme::reversed())
            .highlight_symbol(marker),
        area,
        state,
    );
}

/// `n` rows down from the top of `body`, `h` rows tall (clipped to the body).
pub(crate) fn rows(body: Rect, n: u16, h: u16) -> Rect {
    let y = (body.y + n).min(body.bottom());
    Rect {
        y,
        height: h.min(body.bottom() - y),
        ..body
    }
}

/// Short public key: first 4 bytes in hex.
pub(crate) fn short_key(k: &[u8]) -> String {
    format!("{}", mc_core::Hex(k.get(..4).unwrap_or(k)))
}

pub(crate) fn name_of(b: &[u8]) -> String {
    String::from_utf8_lossy(mc_node::contacts::name_str(b)).into_owned()
}
