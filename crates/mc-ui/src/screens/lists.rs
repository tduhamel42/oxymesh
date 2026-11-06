//! Contacts and channels, read live from the node.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use super::{Soon, draw_list, name_of, rows, step};
use crate::app::{Input, Nav, Screen, Ui};
use crate::chrome::Footer;
use crate::clock;

#[derive(Debug, Default)]
pub struct Contacts {
    state: ListState,
}

/// Favourites first, then the most recently updated.
fn sorted(ui: &Ui<'_>) -> Vec<(String, String)> {
    let c = ui.ctx;
    let mut list: Vec<&mc_node::contacts::Contact> = c.dev.node.contacts.list.iter().collect();
    list.sort_by_key(|k| (k.flags & 1 == 0, core::cmp::Reverse(k.lastmod)));
    list.iter()
        .map(|k| {
            let tag = match k.kind {
                2 => "rpt ",
                3 => "room ",
                4 => "sensor ",
                _ => "",
            };
            let when = if k.lastmod == 0 {
                String::new()
            } else {
                clock::stamp(k.lastmod, c.unix, c.tz_offset_min)
            };
            (name_of(&k.name), format!("{tag}{when}"))
        })
        .collect()
}

impl Screen for Contacts {
    fn title(&self, ui: &Ui<'_>) -> String {
        format!("Contacts {}", ui.ctx.dev.node.contacts.list.len())
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let rows_ = sorted(ui);
        if rows_.is_empty() {
            f.render_widget(
                Paragraph::new("No contacts yet. They appear when their adverts are heard.")
                    .wrap(Wrap { trim: true }),
                rows(body, 1, body.height.saturating_sub(1)),
            );
            return;
        }
        draw_list(f, body, &rows_, &mut self.state, ui);
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let list = sorted(ui);
        if step(&mut self.state, list.len(), input)
            && let Some((name, _)) = list.get(self.state.selected().unwrap_or(0))
        {
            return Nav::Push(Box::new(Soon::new(
                name,
                "Messages, ping, trace, telemetry and the rest arrive in U3.",
            )));
        }
        Nav::Stay
    }

    fn footer(&self, ui: &Ui<'_>) -> Footer {
        if ui.ctx.dev.node.contacts.list.is_empty() {
            Footer::BACK
        } else {
            Footer::LIST
        }
    }
}

#[derive(Debug, Default)]
pub struct Channels {
    state: ListState,
}

fn channels(ui: &Ui<'_>) -> Vec<(String, String)> {
    ui.ctx
        .dev
        .node
        .channels
        .0
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.is_empty())
        .map(|(i, c)| (name_of(&c.name), format!("#{i}")))
        .collect()
}

impl Screen for Channels {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Channels")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let list = channels(ui);
        if list.is_empty() {
            f.render_widget(
                Paragraph::new("No channels.").wrap(Wrap { trim: true }),
                rows(body, 1, body.height.saturating_sub(1)),
            );
            return;
        }
        draw_list(f, body, &list, &mut self.state, ui);
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let list = channels(ui);
        if step(&mut self.state, list.len(), input)
            && let Some((name, _)) = list.get(self.state.selected().unwrap_or(0))
        {
            return Nav::Push(Box::new(Soon::new(
                name,
                "The channel view and compose arrive in U3.",
            )));
        }
        Nav::Stay
    }
}
