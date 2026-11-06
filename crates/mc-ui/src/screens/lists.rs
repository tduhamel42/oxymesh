//! Contacts and channels, read live from the node.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use super::chat::{Compose, Conversation, Target};
use super::contact::{ChannelScreen, ContactScreen};
use super::{draw_list, name_of, rows, step};
use crate::act::Intent;
use crate::app::{Input, Nav, Screen, Ui};
use crate::chrome::Footer;
use crate::clock;

/// What a contact is picked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Write,
    Range,
    Trace,
    Admin,
}

#[derive(Debug, Default)]
pub struct Contacts {
    state: ListState,
    /// Choosing a contact for something rather than browsing.
    pick: Option<Pick>,
}

impl Contacts {
    /// Messages → New message.
    pub fn picker() -> Self {
        Self::pick_for(Pick::Write)
    }

    pub fn pick_for(p: Pick) -> Self {
        Self {
            state: ListState::default(),
            pick: Some(p),
        }
    }
}

/// Favourites first, then the most recently updated.
fn sorted_contacts<'a>(ui: &Ui<'a>) -> Vec<&'a mc_node::contacts::Contact> {
    let mut list: Vec<&mc_node::contacts::Contact> = ui.ctx.dev.node.contacts.list.iter().collect();
    list.sort_by_key(|k| (k.flags & 1 == 0, core::cmp::Reverse(k.lastmod)));
    list
}

fn sorted(ui: &Ui<'_>) -> Vec<(String, String)> {
    let c = ui.ctx;
    sorted_contacts(ui)
        .iter()
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
        if let Some(p) = self.pick {
            String::from(match p {
                Pick::Write => "Write to…",
                Pick::Range => "Range test with…",
                Pick::Trace => "Trace to…",
                Pick::Admin => "Repeater…",
            })
        } else {
            format!("Contacts {}", ui.ctx.dev.node.contacts.list.len())
        }
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
        let list = sorted_contacts(ui);
        if !step(&mut self.state, list.len(), input) {
            return Nav::Stay;
        }
        let Some(c) = list.get(self.state.selected().unwrap_or(0)) else {
            return Nav::Stay;
        };
        match self.pick {
            Some(Pick::Write) => {
                let mut p = [0u8; 6];
                p.copy_from_slice(&c.pub_key[..6]);
                let peer = mc_node::history::Peer::Contact(p);
                return Nav::Replace(Box::new(Compose::to(peer)));
            }
            Some(Pick::Range) => {
                return Nav::Replace(Box::new(super::tools::RangeTest::new(c.pub_key)));
            }
            Some(Pick::Trace) => {
                return Nav::Replace(Box::new(super::tools::Trace::new(c.pub_key)));
            }
            Some(Pick::Admin) => {
                return Nav::Replace(Box::new(super::admin::Admin::new(c.pub_key)));
            }
            None => {}
        }
        Nav::Push(Box::new(ContactScreen::new(c.pub_key)))
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

/// (slot, name, unread or empty).
fn channels(ui: &Ui<'_>) -> Vec<(u8, String, String)> {
    let h = ui.ctx.dev.history();
    ui.ctx
        .dev
        .node
        .channels
        .0
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.is_empty())
        .map(|(i, c)| {
            let idx = i as u8;
            let unread = h.unread_with(mc_node::history::Peer::Channel(idx));
            let detail = if unread > 0 {
                format!("{}{unread}", ui.sym.unread)
            } else {
                String::new()
            };
            (idx, name_of(&c.name), detail)
        })
        .collect()
}

impl Screen for Channels {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Channels")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let mut rows_: Vec<(String, String)> =
            alloc::vec![(String::from("+ Add #channel"), String::new())];
        rows_.extend(channels(ui).into_iter().map(|(_, n, d)| (n, d)));
        draw_list(f, body, &rows_, &mut self.state, ui);
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let list = channels(ui);
        if !step(&mut self.state, list.len() + 1, input) {
            return Nav::Stay;
        }
        match self.state.selected().unwrap_or(0) {
            0 => Nav::Push(Box::new(Compose::new(Target::Hashtag, ""))),
            i => match list.get(i - 1) {
                Some((idx, _, d)) if !d.is_empty() => Nav::PushWith(
                    Box::new(Conversation::new(mc_node::history::Peer::Channel(*idx))),
                    Intent::MarkRead(mc_node::history::Peer::Channel(*idx)),
                ),
                Some((idx, _, _)) => Nav::Push(Box::new(ChannelScreen::new(*idx))),
                None => Nav::Stay,
            },
        }
    }
}
