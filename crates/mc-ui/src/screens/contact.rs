//! One contact, one channel: what we know about it and what can be done with it
//! (`docs/spec/ui.md` §6).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mc_node::contacts::{Contact, PATH_UNKNOWN};
use mc_node::history::Peer;
use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::Paragraph;

use super::chat::{Compose, Conversation};
use super::{Card, draw_list, name_of, rows, short_key, step};
use crate::act::{Intent, Outcome};
use crate::app::{Confirm, Input, Nav, Screen, Ui};
use crate::clock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    Write,
    Messages,
    Ping,
    Favourite,
    ResetRoute,
    Trace,
    Share,
    Remove,
}

const ACTS: [(Act, &str); 8] = [
    (Act::Write, "Write"),
    (Act::Messages, "Messages"),
    (Act::Ping, "Ping"),
    (Act::Favourite, "Favourite"),
    (Act::ResetRoute, "Reset route"),
    (Act::Trace, "Trace route"),
    (Act::Share, "Share (QR)"),
    (Act::Remove, "Remove"),
];

#[derive(Debug)]
pub struct ContactScreen {
    key: [u8; 32],
    state: ListState,
    /// The last ping from this screen: its request tag.
    ping: Option<u32>,
}

impl ContactScreen {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            state: ListState::default().with_selected(Some(0)),
            ping: None,
        }
    }

    fn contact<'a>(&self, ui: &Ui<'a>) -> Option<&'a Contact> {
        let c = &ui.ctx.dev.node.contacts;
        c.find(&self.key).and_then(|s| c.get(s))
    }

    fn peer(&self) -> Peer {
        let mut p = [0u8; 6];
        p.copy_from_slice(&self.key[..6]);
        Peer::Contact(p)
    }
}

pub(crate) fn kind_name(kind: u8) -> &'static str {
    match kind {
        1 => "Chat",
        2 => "Repeater",
        3 => "Room",
        4 => "Sensor",
        _ => "Node",
    }
}

fn route_of(c: &Contact) -> String {
    if c.out_path_len == PATH_UNKNOWN {
        return String::from("no route yet");
    }
    match c.out_path_len & 63 {
        0 => String::from("direct"),
        1 => String::from("1 hop"),
        n => format!("{n} hops"),
    }
}

impl Screen for ContactScreen {
    fn title(&self, ui: &Ui<'_>) -> String {
        self.contact(ui)
            .map(|c| name_of(&c.name))
            .unwrap_or_else(|| String::from("Contact"))
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let Some(c) = self.contact(ui) else {
            f.render_widget(Paragraph::new("Contact removed."), body);
            return;
        };
        let ctx = ui.ctx;
        let fav = c.flags & 1 != 0;
        let mut info = alloc::vec![
            format!("{} {} {}", kind_name(c.kind), ui.sym.dot, route_of(c)),
            format!(
                "Key {} {} {}",
                short_key(&c.pub_key),
                ui.sym.dot,
                if c.lastmod == 0 {
                    String::from("never heard")
                } else {
                    clock::stamp(c.lastmod, ctx.unix, ctx.tz_offset_min)
                }
            ),
        ];
        if let Some(tag) = self.ping {
            info.push(match ctx.dev.request_reply(tag) {
                Some(r) if r.ok => {
                    format!("Ping: {}.{:02} s", r.rtt_ms / 1000, r.rtt_ms % 1000 / 10)
                }
                Some(_) => String::from("Ping: no answer"),
                None => String::from("Ping: waiting…"),
            });
        }
        let n = info.len() as u16;
        let lines: Vec<Line<'_>> = info.into_iter().map(Line::from).collect();
        f.render_widget(Paragraph::new(lines), rows(body, 0, n));
        let acts: Vec<(String, String)> = ACTS
            .iter()
            .map(|(a, label)| {
                let detail = match a {
                    Act::Favourite => String::from(if fav { "yes" } else { "no" }),
                    Act::Messages => match ctx.dev.history().unread_with(self.peer()) {
                        0 => String::new(),
                        u => format!("{}{u}", ui.sym.unread),
                    },
                    _ => String::new(),
                };
                (String::from(*label), detail)
            })
            .collect();
        draw_list(
            f,
            rows(body, n + 1, body.height.saturating_sub(n + 1)),
            &acts,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        if !step(&mut self.state, ACTS.len(), input) {
            return Nav::Stay;
        }
        let Some(c) = self.contact(ui) else {
            return Nav::Pop;
        };
        let (act, _) = ACTS[self.state.selected().unwrap_or(0) % ACTS.len()];
        let key = self.key;
        match act {
            Act::Write => Nav::Push(Box::new(Compose::to(self.peer()))),
            Act::Messages => Nav::PushWith(
                Box::new(Conversation::new(self.peer())),
                Intent::MarkRead(self.peer()),
            ),
            Act::Ping => Nav::Intent(Intent::Ping(key)),
            Act::Favourite => {
                let on = c.flags & 1 == 0;
                Nav::IntentToast(
                    Intent::SetFavourite(key, on),
                    if on { "Favourite" } else { "Not a favourite" },
                )
            }
            Act::ResetRoute => Nav::IntentToast(Intent::ResetPath(key), "Route reset"),
            Act::Trace => Nav::Push(Box::new(super::tools::Trace::new(key))),
            Act::Share => Nav::Push(Box::new(Card::of(&name_of(&c.name), &c.pub_key))),
            Act::Remove => Nav::Confirm(
                Confirm::new(
                    "Remove",
                    "Remove this contact from the device?",
                    Intent::RemoveContact(key),
                )
                .with_toast("Removed"),
            ),
        }
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        match (intent, outcome) {
            (Intent::Ping(_), Outcome::Request(tag)) => self.ping = Some(*tag),
            (Intent::RemoveContact(_), Outcome::Ok) => return Nav::Pop,
            _ => {}
        }
        Nav::Stay
    }
}

#[derive(Debug)]
pub struct ChannelScreen {
    idx: u8,
    state: ListState,
}

impl ChannelScreen {
    pub fn new(idx: u8) -> Self {
        Self {
            idx,
            state: ListState::default().with_selected(Some(0)),
        }
    }
}

const CHANNEL_ACTS: [&str; 3] = ["Messages", "Write", "Remove"];

impl Screen for ChannelScreen {
    fn title(&self, ui: &Ui<'_>) -> String {
        ui.ctx
            .dev
            .node
            .channels
            .0
            .get(usize::from(self.idx))
            .map(|c| name_of(&c.name))
            .unwrap_or_default()
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let peer = Peer::Channel(self.idx);
        let h = ui.ctx.dev.history();
        let info = format!(
            "Slot {} {} {} messages {} {} unread",
            self.idx,
            ui.sym.dot,
            h.with(peer).count(),
            ui.sym.dot,
            h.unread_with(peer)
        );
        f.render_widget(
            Paragraph::new(crate::chrome::fit(&info, usize::from(body.width))),
            rows(body, 0, 1),
        );
        let acts: Vec<(String, String)> = CHANNEL_ACTS
            .iter()
            .map(|l| (String::from(*l), String::new()))
            .collect();
        draw_list(
            f,
            rows(body, 2, body.height.saturating_sub(2)),
            &acts,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        if !step(&mut self.state, CHANNEL_ACTS.len(), input) {
            return Nav::Stay;
        }
        let peer = Peer::Channel(self.idx);
        match self.state.selected().unwrap_or(0) {
            0 => Nav::PushWith(Box::new(Conversation::new(peer)), Intent::MarkRead(peer)),
            1 => Nav::Push(Box::new(Compose::to(peer))),
            _ => Nav::Confirm(
                Confirm::new(
                    "Remove",
                    "Leave this channel?",
                    Intent::RemoveChannel(self.idx),
                )
                .with_toast("Removed"),
            ),
        }
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        if matches!(intent, Intent::RemoveChannel(_)) && *outcome == Outcome::Ok {
            return Nav::Pop;
        }
        Nav::Stay
    }
}
