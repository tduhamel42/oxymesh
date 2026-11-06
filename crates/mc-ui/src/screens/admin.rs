//! Discover and Repeater admin (`docs/spec/ui.md` §6): zero-hop discovery of the repeaters
//! around, then login, status and neighbours of one repeater, in the formats of MeshCore's
//! `simple_repeater` (v1.18).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mc_node::device::Discovered;
use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use super::chat::{Compose, Target};
use super::tools::bars;
use super::{draw_list, name_of, rows, short_key, step};
use crate::act::{Intent, Outcome};
use crate::app::{Input, Nav, Screen, Ui};
use crate::chrome::{self, Footer};

/// At most one discovery every 5 s (`docs/spec/ui.md` §6).
const DISCOVER_EVERY_MS: u64 = 5_000;

fn snr1(snr4: i8) -> String {
    let s = i16::from(snr4);
    let a = s.unsigned_abs();
    format!(
        "{}{}.{}",
        if s < 0 { "-" } else { "" },
        a / 4,
        a % 4 * 25 / 10
    )
}

/// Name for a key prefix: a contact, a heard node, or the hex prefix.
fn name_for(ui: &Ui<'_>, prefix: &[u8]) -> String {
    let dev = ui.ctx.dev;
    if prefix.is_empty() {
        return String::from("?");
    }
    if dev.node.id.pub_key().starts_with(prefix) {
        return String::from("You");
    }
    if let Some(c) = dev
        .node
        .contacts
        .list
        .iter()
        .find(|c| c.pub_key.starts_with(prefix))
    {
        return name_of(&c.name);
    }
    if let Some(n) = dev
        .heard()
        .sorted()
        .into_iter()
        .find(|n| n.key.starts_with(prefix))
    {
        let own = name_of(&n.name);
        if !own.is_empty() {
            return own;
        }
    }
    short_key(prefix)
}

// ------------------------------------------------------------------ discover

#[derive(Debug, Default)]
pub struct Discover {
    state: ListState,
    last_ms: Option<u64>,
}

fn found(ui: &Ui<'_>) -> Vec<Discovered> {
    let mut v: Vec<Discovered> = ui.ctx.dev.discovered().to_vec();
    v.sort_by_key(|d| core::cmp::Reverse(d.snr_back));
    v
}

impl Screen for Discover {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Discover")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let list = found(ui);
        let head = match (self.last_ms, list.len()) {
            (None, _) => String::from("Hold A to find the repeaters within direct reach."),
            (Some(_), 0) => String::from("Searching… nothing yet."),
            (Some(_), n) => format!("{n} found. SNR in dB: theirs/ours."),
        };
        f.render_widget(
            Paragraph::new(head).wrap(Wrap { trim: true }),
            rows(body, 0, 2),
        );
        let items: Vec<(String, String)> = list
            .iter()
            .map(|d| {
                let prefix = d.key.get(..usize::from(d.key_len)).unwrap_or(&[]);
                (
                    name_for(ui, prefix),
                    format!(
                        "{} {}/{}",
                        bars(d.snr_back),
                        d.snr_there / 4,
                        d.snr_back / 4
                    ),
                )
            })
            .collect();
        draw_list(
            f,
            rows(body, 3, body.height.saturating_sub(3)),
            &items,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        if input == Input::Open {
            let now = ui.ctx.now_ms;
            if self.last_ms.is_some_and(|t| now < t + DISCOVER_EVERY_MS) {
                return Nav::Toast(String::from("Wait a few seconds"));
            }
            self.last_ms = Some(now);
            return Nav::Intent(Intent::Discover);
        }
        let n = ui.ctx.dev.discovered().len();
        let _ = step(&mut self.state, n, input);
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: Some("next"),
            a_hold: Some("search"),
            b_click: Some("back"),
            b_hold: None,
        }
    }
}

// ------------------------------------------------------------------ repeater admin

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Login {
    No,
    Waiting(u32),
}

#[derive(Debug)]
pub struct Admin {
    key: [u8; 32],
    state: ListState,
    login: Login,
}

const ADMIN_ACTS: [&str; 4] = [
    "Login as guest",
    "Login with password",
    "Status",
    "Neighbours",
];

impl Admin {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            state: ListState::default().with_selected(Some(0)),
            login: Login::No,
        }
    }

    /// `Some(true)`: logged in as admin, `Some(false)`: as guest; `None`: not (yet).
    fn logged(&self, ui: &Ui<'_>) -> Option<Result<bool, ()>> {
        let Login::Waiting(tag) = self.login else {
            return None;
        };
        let r = ui.ctx.dev.request_reply(tag)?;
        // Login block: [0] 0 = OK, [2] admin.
        Some(if r.ok && r.data.first() == Some(&0) {
            Ok(r.data.get(2) == Some(&1))
        } else {
            Err(())
        })
    }
}

impl Screen for Admin {
    fn title(&self, ui: &Ui<'_>) -> String {
        name_for(ui, &self.key)
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let state = match (self.login, self.logged(ui)) {
            (Login::No, _) => String::from("Not logged in."),
            (_, None) => String::from("Logging in…"),
            (_, Some(Ok(true))) => String::from("Logged in as admin."),
            (_, Some(Ok(false))) => String::from("Logged in as guest."),
            (_, Some(Err(()))) => String::from("Login failed: wrong password or no answer."),
        };
        f.render_widget(
            Paragraph::new(state).wrap(Wrap { trim: true }),
            rows(body, 0, 2),
        );
        let items: Vec<(String, String)> = ADMIN_ACTS
            .iter()
            .map(|a| (String::from(*a), String::new()))
            .collect();
        draw_list(
            f,
            rows(body, 3, body.height.saturating_sub(3)),
            &items,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        if !step(&mut self.state, ADMIN_ACTS.len(), input) {
            return Nav::Stay;
        }
        let key = self.key;
        match self.state.selected().unwrap_or(0) {
            0 => Nav::Intent(Intent::Login {
                key,
                password: String::new(),
            }),
            1 => Nav::Push(Box::new(Compose::new(Target::Password(key), ""))),
            2 => Nav::PushWith(
                Box::new(RepeaterStatus::default()),
                Intent::RepeaterStatus(key),
            ),
            _ => Nav::PushWith(Box::new(Neighbours::default()), Intent::Neighbours(key)),
        }
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        if let (Intent::Login { .. }, Outcome::Request(tag)) = (intent, outcome) {
            self.login = Login::Waiting(*tag);
        }
        Nav::Stay
    }
}

/// A screen showing the answer to one request; sent by the screen that opened it.
#[derive(Debug, Default)]
pub struct RepeaterStatus {
    tag: Option<u32>,
}

fn le16(d: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([
        d.get(i).copied().unwrap_or(0),
        d.get(i + 1).copied().unwrap_or(0),
    ])
}

fn le32(d: &[u8], i: usize) -> u32 {
    u32::from(le16(d, i)) | (u32::from(le16(d, i + 2)) << 16)
}

fn duration(s: u32) -> String {
    match s {
        0..3600 => format!("{} min", s / 60),
        3600..86_400 => format!("{} h {:02} min", s / 3600, s / 60 % 60),
        _ => format!("{} d {} h", s / 86_400, s / 3600 % 24),
    }
}

impl Screen for RepeaterStatus {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Repeater status")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let w = usize::from(body.width);
        let reply = self.tag.and_then(|t| ui.ctx.dev.request_reply(t));
        let Some(r) = reply else {
            f.render_widget(Paragraph::new("Asking…"), rows(body, 1, 1));
            return;
        };
        if !r.ok {
            f.render_widget(
                Paragraph::new("No answer. Log in first, then try again.")
                    .wrap(Wrap { trim: true }),
                rows(body, 1, 2),
            );
            return;
        }
        // RepeaterStats (`simple_repeater/MyMesh.h:44-60`), little-endian.
        let d = &r.data;
        let mv = le16(d, 0);
        let l = alloc::vec![
            chrome::spread(
                "Battery",
                &format!("{}.{:02} V", mv / 1000, mv % 1000 / 10),
                w
            ),
            chrome::spread("Up", &duration(le32(d, 20)), w),
            chrome::spread("Queue", &format!("{}", le16(d, 2)), w),
            chrome::spread("Noise floor", &format!("{} dBm", le16(d, 4) as i16), w),
            chrome::spread(
                "Last",
                &format!(
                    "{} dB {} dBm",
                    snr1(le16(d, 42) as i16 as i8),
                    le16(d, 6) as i16
                ),
                w,
            ),
            chrome::spread("Received", &format!("{}", le32(d, 8)), w),
            chrome::spread("Sent", &format!("{}", le32(d, 12)), w),
            chrome::spread(
                "Airtime TX/RX",
                &format!("{}/{} s", le32(d, 16), le32(d, 48)),
                w
            ),
            chrome::spread(
                "Duplicates",
                &format!("{}", u32::from(le16(d, 44)) + u32::from(le16(d, 46))),
                w
            ),
            chrome::spread("Errors", &format!("{}", le32(d, 52)), w),
            chrome::spread(
                "Answer",
                &format!("{}.{:02} s", r.rtt_ms / 1000, r.rtt_ms % 1000 / 10),
                w
            ),
        ];
        let lines: Vec<Line<'_>> = l.into_iter().map(Line::from).collect();
        f.render_widget(Paragraph::new(lines), body);
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        if let (Intent::RepeaterStatus(_), Outcome::Request(tag)) = (intent, outcome) {
            self.tag = Some(*tag);
        }
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }
}

#[derive(Debug, Default)]
pub struct Neighbours {
    tag: Option<u32>,
    state: ListState,
}

impl Screen for Neighbours {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Neighbours")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let reply = self.tag.and_then(|t| ui.ctx.dev.request_reply(t));
        let Some(r) = reply else {
            f.render_widget(Paragraph::new("Asking…"), rows(body, 1, 1));
            return;
        };
        if !r.ok {
            f.render_widget(
                Paragraph::new("No answer. Log in first, then try again.")
                    .wrap(Wrap { trim: true }),
                rows(body, 1, 2),
            );
            return;
        }
        // [total u16][count u16] then per neighbour: key prefix (6), seconds ago (u32), SNR ×4.
        let d = &r.data;
        let (total, count) = (le16(d, 0), usize::from(le16(d, 2)));
        let items: Vec<(String, String)> = d
            .get(4..)
            .unwrap_or(&[])
            .as_chunks::<11>()
            .0
            .iter()
            .take(count)
            .map(|e| {
                let ago = le32(e, 6);
                let snr = e.get(10).copied().unwrap_or(0) as i8;
                (
                    name_for(ui, &e[..6]),
                    format!(
                        "{} {}",
                        bars(snr),
                        if ago < 3600 {
                            format!("{}m", ago / 60)
                        } else {
                            format!("{}h", ago / 3600)
                        }
                    ),
                )
            })
            .collect();
        f.render_widget(
            Paragraph::new(format!("{total} heard by the repeater")),
            rows(body, 0, 1),
        );
        draw_list(
            f,
            rows(body, 2, body.height.saturating_sub(2)),
            &items,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        let n = self
            .tag
            .and_then(|t| ui.ctx.dev.request_reply(t))
            .map(|r| usize::from(le16(&r.data, 2)))
            .unwrap_or(0);
        let _ = step(&mut self.state, n, input);
        Nav::Stay
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        if let (Intent::Neighbours(_), Outcome::Request(tag)) = (intent, outcome) {
            self.tag = Some(*tag);
        }
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: Some("next"),
            a_hold: None,
            b_click: Some("back"),
            b_hold: None,
        }
    }
}
