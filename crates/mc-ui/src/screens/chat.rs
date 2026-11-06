//! Messages: the conversations, one conversation, and writing a message
//! (`docs/spec/ui.md` §6-7).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mc_node::history::{Msg, Peer, Status};
use ratatui_core::layout::{Alignment, Rect};
use ratatui_core::style::Style;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use super::{Contacts, draw_list, peer_name, rows, step};
use crate::act::{Intent, Outcome};
use crate::app::{Input, Nav, Screen, Ui};
use crate::chrome::{self, Footer};
use crate::clock;
use crate::keyboard::{Cand, Key, Keyboard, Mode, recent_words};
use crate::theme;

/// Longest message text (`MAX_TEXT_LEN`, 10 AES blocks).
pub const TEXT_MAX: usize = 160;

// ------------------------------------------------------------------ conversations

#[derive(Debug, Default)]
pub struct Messages {
    state: ListState,
}

/// Unread first, then the most recent.
fn conversations(ui: &Ui<'_>) -> Vec<(Peer, String, String)> {
    let h = ui.ctx.dev.history();
    let mut v: Vec<(Peer, u32, usize)> = h
        .conversations()
        .iter()
        .map(|(p, m)| (*p, m.seq, h.unread_with(*p)))
        .collect();
    v.sort_by_key(|(_, seq, unread)| (*unread == 0, core::cmp::Reverse(*seq)));
    v.into_iter()
        .map(|(p, seq, unread)| {
            let detail = if unread > 0 {
                format!("{}{unread}", ui.sym.unread)
            } else {
                h.get(seq)
                    .map(|m| clock::stamp(m.at, ui.ctx.unix, ui.ctx.tz_offset_min))
                    .unwrap_or_default()
            };
            (p, peer_name(ui, p), detail)
        })
        .collect()
}

impl Screen for Messages {
    fn title(&self, _ui: &Ui<'_>) -> String {
        // The header already counts the unread messages.
        String::from("Messages")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let mut rows_: Vec<(String, String)> =
            alloc::vec![(String::from("+ New message"), String::new())];
        rows_.extend(conversations(ui).into_iter().map(|(_, n, d)| (n, d)));
        draw_list(f, body, &rows_, &mut self.state, ui);
        if rows_.len() == 1 {
            f.render_widget(
                Paragraph::new("No messages yet.").wrap(Wrap { trim: true }),
                rows(body, 2, body.height.saturating_sub(2)),
            );
        }
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let conv = conversations(ui);
        if !step(&mut self.state, conv.len() + 1, input) {
            return Nav::Stay;
        }
        match self.state.selected().unwrap_or(0) {
            0 => Nav::Push(Box::new(Contacts::picker())),
            i => match conv.get(i - 1) {
                Some((peer, _, _)) => {
                    Nav::PushWith(Box::new(Conversation::new(*peer)), Intent::MarkRead(*peer))
                }
                None => Nav::Stay,
            },
        }
    }
}

// ------------------------------------------------------------------ one conversation

/// One conversation; the newest messages at the bottom, A scrolls.
#[derive(Debug)]
pub struct Conversation {
    peer: Peer,
    /// Lines scrolled up from the bottom.
    up: usize,
    /// Lines last drawn, to bound scrolling.
    total: usize,
    page: usize,
}

impl Conversation {
    pub fn new(peer: Peer) -> Self {
        Self {
            peer,
            up: 0,
            total: 0,
            page: 1,
        }
    }
}

fn route(m: &Msg) -> String {
    match m.path_len {
        0xFF => String::from("direct"),
        p => match p & 63 {
            0 => String::from("direct"),
            1 => String::from("1 hop"),
            n => format!("{n} hops"),
        },
    }
}

fn status(m: &Msg) -> String {
    match m.status {
        Status::Sending { attempt: 0 } => String::from("sending…"),
        Status::Sending { attempt: 3 } => String::from("flood…"),
        Status::Sending { attempt } => format!("try {}…", attempt + 1),
        Status::Delivered { rtt_ms } => {
            format!("delivered {}.{} s", rtt_ms / 1000, rtt_ms % 1000 / 100)
        }
        Status::Sent => String::from("sent"),
        Status::Failed => String::from("failed"),
        Status::Unread | Status::Read => String::new(),
    }
}

/// The lines of the whole conversation: a header per message, then its wrapped text;
/// received on the left, sent on the right.
fn lines(ui: &Ui<'_>, peer: Peer, width: usize) -> Vec<(String, Alignment, Style)> {
    let c = ui.ctx;
    let mut out = Vec::new();
    for m in c.dev.history().with(peer) {
        let when = clock::stamp(m.at, c.unix, c.tz_offset_min);
        let text = String::from_utf8_lossy(&m.text).into_owned();
        let (head, body, align) = if m.outgoing {
            (
                format!("{when} {} {}", ui.sym.dot, status(m)),
                text,
                Alignment::Right,
            )
        } else if let Peer::Channel(_) = peer {
            // "Name: text" on channels.
            let (who, t) = match text.split_once(": ") {
                Some((w, t)) => (String::from(w), String::from(t)),
                None => (String::from("?"), text.clone()),
            };
            (format!("{who} {} {when}", ui.sym.dot), t, Alignment::Left)
        } else {
            (
                format!("{when} {} {}", ui.sym.dot, route(m)),
                text,
                Alignment::Left,
            )
        };
        out.push((chrome::fit(&head, width), align, theme::label()));
        for l in chrome::wrap(&body, width.saturating_sub(2)) {
            out.push((l, align, Style::new()));
        }
        out.push((String::new(), Alignment::Left, Style::new()));
    }
    out.pop();
    out
}

impl Screen for Conversation {
    fn title(&self, ui: &Ui<'_>) -> String {
        peer_name(ui, self.peer)
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let all = lines(ui, self.peer, usize::from(body.width));
        let h = usize::from(body.height);
        self.total = all.len();
        self.page = (h / 2).max(1);
        self.up = self.up.min(all.len().saturating_sub(h));
        if all.is_empty() {
            f.render_widget(
                Paragraph::new("No messages yet. Hold A to write.").wrap(Wrap { trim: true }),
                rows(body, 1, body.height.saturating_sub(1)),
            );
            return;
        }
        let end = all.len() - self.up;
        let start = end.saturating_sub(h);
        for (i, (text, align, style)) in all[start..end].iter().enumerate() {
            f.render_widget(
                Paragraph::new(Line::from(text.as_str()))
                    .alignment(*align)
                    .style(*style),
                rows(body, i as u16, 1),
            );
        }
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        match input {
            // Down: towards the newest.
            Input::Next(n) => self.up = self.up.saturating_sub(self.page * usize::from(n)),
            Input::Prev => self.up = (self.up + self.page).min(self.total),
            Input::Open => return Nav::Push(Box::new(Compose::to(self.peer))),
        }
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: Some("down"),
            a_hold: Some("write"),
            b_click: Some("back"),
            b_hold: None,
        }
    }

    fn peer(&self) -> Option<Peer> {
        Some(self.peer)
    }
}

// ------------------------------------------------------------------ writing

/// What the text is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Peer(Peer),
    /// A new `#hashtag` channel.
    Hashtag,
    /// The node's name; `setup`: a step of the first-boot setup.
    Name {
        setup: bool,
    },
    /// A server's password, then log in.
    Password([u8; 32]),
    /// The radio frequency in kHz (hold-to-confirm).
    Frequency,
    /// A fixed Bluetooth PIN, six digits.
    Pin,
}

#[derive(Debug)]
pub struct Compose {
    target: Target,
    kb: Option<Keyboard>,
    initial: String,
}

impl Compose {
    pub fn to(peer: Peer) -> Self {
        Self {
            target: Target::Peer(peer),
            kb: None,
            initial: String::new(),
        }
    }

    pub fn new(target: Target, initial: &str) -> Self {
        Self {
            target,
            kb: None,
            initial: String::from(initial),
        }
    }

    fn mode(&self, ui: &Ui<'_>) -> Mode {
        match self.target {
            Target::Peer(Peer::Contact(_)) => Mode::Message { limit: TEXT_MAX },
            Target::Peer(Peer::Channel(_)) => {
                // The sender's name and ": " travel in the same text.
                let name = mc_node::contacts::name_str(&ui.ctx.dev.node.prefs.name).len();
                Mode::Message {
                    limit: TEXT_MAX.saturating_sub(name + 2),
                }
            }
            Target::Hashtag => Mode::Hashtag,
            Target::Name { .. } => Mode::Name,
            Target::Password(_) => Mode::Password,
            Target::Frequency => Mode::Digits { limit: 7 },
            Target::Pin => Mode::Digits { limit: 6 },
        }
    }

    /// Created on first use: the recent words come from the history.
    fn kb(&mut self, ui: &Ui<'_>) -> &mut Keyboard {
        let mode = self.mode(ui);
        if self.initial.is_empty()
            && let Target::Name { .. } = self.target
        {
            self.initial = super::name_of(&ui.ctx.dev.node.prefs.name);
        }
        let initial = &self.initial;
        self.kb.get_or_insert_with(|| {
            let h = ui.ctx.dev.history();
            let sent: Vec<&[u8]> = h
                .iter()
                .filter(|m| m.outgoing)
                .map(|m| m.text.as_slice())
                .collect();
            Keyboard::new(mode, initial, recent_words(sent.into_iter()))
        })
    }

    fn finish(&self, text: String, ui: &Ui<'_>) -> Nav {
        match self.target {
            Target::Peer(Peer::Contact(prefix)) => {
                let contacts = &ui.ctx.dev.node.contacts;
                match contacts.find_prefix(&prefix).and_then(|s| contacts.get(s)) {
                    Some(c) => Nav::PopWith(
                        Intent::SendText {
                            key: c.pub_key,
                            text,
                        },
                        None,
                    ),
                    None => Nav::Toast(String::from("Unknown contact")),
                }
            }
            Target::Peer(Peer::Channel(idx)) => {
                Nav::PopWith(Intent::SendChannel { idx, text }, None)
            }
            Target::Hashtag => Nav::PopWith(Intent::AddChannel(text), Some("Channel added")),
            Target::Name { setup: false } => {
                Nav::PopWith(Intent::SetName(text), Some("Name saved"))
            }
            Target::Name { setup: true } => Nav::Intent(Intent::SetName(text)),
            Target::Frequency => match text.parse::<u32>() {
                Ok(f) if (150_000..=2_500_000).contains(&f) => {
                    let r = &ui.ctx.dev.node.prefs.radio;
                    Nav::Confirm(
                        crate::app::Confirm::new(
                            "Frequency",
                            &format!(
                                "Every node you talk to must use the same frequency. Switch to {}.{:03} MHz.",
                                f / 1000,
                                f % 1000
                            ),
                            Intent::SetRadio {
                                freq_khz: f,
                                bw_hz: r.bw_hz,
                                sf: r.sf,
                                cr: r.cr,
                            },
                        )
                        .with_toast("Frequency saved"),
                    )
                }
                _ => Nav::Toast(String::from("150000 to 2500000 kHz")),
            },
            Target::Pin => match text.parse::<u32>() {
                Ok(p) if text.len() == 6 && (100_000..=999_999).contains(&p) => Nav::PopWith(
                    Intent::SetPin(p),
                    Some("PIN saved, used from the next reboot"),
                ),
                _ => Nav::Toast(String::from("Six digits, not starting with 0")),
            },
            Target::Password(key) => Nav::PopWith(
                Intent::Login {
                    key,
                    password: text,
                },
                None,
            ),
        }
    }
}

impl Screen for Compose {
    fn title(&self, ui: &Ui<'_>) -> String {
        match self.target {
            Target::Peer(p) => format!("To {}", peer_name(ui, p)),
            Target::Hashtag => String::from("New #channel"),
            Target::Name { setup: true } => String::from("2/3 Name"),
            Target::Name { setup: false } => String::from("Your name"),
            Target::Password(_) => String::from("Password"),
            Target::Frequency => String::from("Frequency (kHz)"),
            Target::Pin => String::from("Bluetooth PIN"),
        }
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let w = usize::from(body.width);
        let hashtag = self.target == Target::Hashtag;
        let kb = self.kb(ui);
        // Text box: the last lines of the wrapped text, with a cursor.
        let shown = if hashtag {
            format!("#{}_", kb.text)
        } else {
            format!("{}_", kb.text)
        };
        let box_h = (body.height / 2).saturating_sub(1).max(1);
        let wrapped = chrome::wrap(&shown, w);
        let skip = wrapped.len().saturating_sub(usize::from(box_h));
        let text: Vec<Line<'_>> = wrapped.into_iter().skip(skip).map(Line::from).collect();
        f.render_widget(Paragraph::new(text), rows(body, 0, box_h));
        // Rule with the byte count.
        let count = format!(" {}/{}", kb.bytes(), kb.mode.limit());
        let rule = format!(
            "{}{count}",
            "─".repeat(w.saturating_sub(count.chars().count()))
        );
        f.render_widget(Paragraph::new(rule), rows(body, box_h, 1));
        // Candidates, flowed into the rows left; the page holding the selection.
        let area = rows(body, box_h + 1, body.height.saturating_sub(box_h + 1));
        let mut lines: Vec<Vec<(usize, String)>> = alloc::vec![Vec::new()];
        let mut len = 0usize;
        for (i, c) in kb.candidates().iter().enumerate() {
            let label = chrome::fit(&c.label(kb.mode), w);
            let l = label.chars().count();
            if len > 0 && len + 1 + l > w {
                lines.push(Vec::new());
                len = 0;
            }
            len += l + usize::from(len > 0);
            if let Some(last) = lines.last_mut() {
                last.push((i, label));
            }
        }
        let per_page = usize::from(area.height).max(1);
        let sel_line = lines
            .iter()
            .position(|l| l.iter().any(|(i, _)| *i == kb.sel))
            .unwrap_or(0);
        let first = sel_line / per_page * per_page;
        for (row, line) in lines.iter().skip(first).take(per_page).enumerate() {
            let mut spans = Vec::new();
            for (k, (i, label)) in line.iter().enumerate() {
                if k > 0 {
                    spans.push(ratatui_core::text::Span::raw(" "));
                }
                let style = if *i == kb.sel {
                    theme::reversed()
                } else {
                    Style::new()
                };
                spans.push(ratatui_core::text::Span::styled(label.clone(), style));
            }
            f.render_widget(Paragraph::new(Line::from(spans)), rows(area, row as u16, 1));
        }
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let kb = self.kb(ui);
        match input {
            Input::Next(n) => kb.next(usize::from(n)),
            Input::Prev => kb.prev(),
            Input::Open => {
                if let Key::Done(text) = kb.insert() {
                    return self.finish(text, ui);
                }
            }
        }
        Nav::Stay
    }

    fn back(&mut self, ui: &Ui<'_>) -> Option<Nav> {
        match self.kb(ui).delete() {
            Key::Cancel => Some(match self.target {
                // Setup: keep the current name and go on.
                Target::Name { setup: true } => Nav::Replace(Box::new(super::setup::SetupTime)),
                _ => Nav::Pop,
            }),
            _ => Some(Nav::Stay),
        }
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        let empty = self.kb.as_ref().is_none_or(|k| k.text.is_empty());
        let done = self
            .kb
            .as_ref()
            .and_then(|k| k.selected())
            .is_some_and(|c| *c == Cand::Done);
        Footer {
            a_click: Some("next"),
            a_hold: Some(if done { "done" } else { "add" }),
            b_click: Some(if empty { "cancel" } else { "delete" }),
            b_hold: None,
        }
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        // Setup: the name is saved, next step.
        if matches!(intent, Intent::SetName(_))
            && self.target == (Target::Name { setup: true })
            && *outcome == Outcome::Ok
        {
            return Nav::Replace(Box::new(super::setup::SetupTime));
        }
        if matches!(intent, Intent::SetRadio { .. })
            && self.target == Target::Frequency
            && *outcome == Outcome::Ok
        {
            return Nav::Pop;
        }
        Nav::Stay
    }

    fn keyboard(&self) -> Option<&Keyboard> {
        self.kb.as_ref()
    }
}
