//! Home: the always-on face (big clock, date, what is new) and the launcher.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::{Alignment, Rect};
use ratatui_core::terminal::Frame;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::Paragraph;

use super::{Dest, draw_list, name_of, open, rows, short_key, step};
use crate::app::{Input, Nav, Paint, Screen, Ui};
use crate::chrome::Footer;
use crate::clock;
use crate::panel::COMPACT;
use crate::theme;

const LAUNCHER: [(&str, Dest); 5] = [
    ("Messages", Dest::Messages),
    ("Contacts", Dest::Contacts),
    ("Channels", Dest::Channels),
    ("Tools", Dest::Tools),
    ("Settings", Dest::Settings),
];

/// The advertised name, else the contact's, else the short key.
pub(crate) fn heard_name(ui: &Ui<'_>, n: &mc_node::heard::HeardNode) -> String {
    let own = name_of(&n.name);
    if !own.is_empty() {
        return own;
    }
    let contacts = &ui.ctx.dev.node.contacts;
    contacts
        .find(&n.key)
        .and_then(|s| contacts.get(s))
        .map(|c| name_of(&c.name))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| short_key(&n.key))
}

#[derive(Debug, Default)]
pub struct Home {
    state: ListState,
}

impl Home {
    fn clock_rows(ui: &Ui<'_>) -> u16 {
        if ui.grid == COMPACT { 5 } else { 4 }
    }

    fn info(ui: &Ui<'_>) -> Vec<String> {
        let c = ui.ctx;
        let dev = c.dev;
        let new = dev.unread_alerting();
        let unread = match new {
            0 => String::from("No new messages"),
            1 => String::from("1 new message"),
            n => format!("{n} new messages"),
        };
        let heard = match dev.heard().last() {
            Some(n) => format!(
                "Heard {} {} {}",
                heard_name(ui, n),
                ui.sym.dot,
                clock::stamp(n.at, c.unix, c.tz_offset_min)
            ),
            None => String::from("Nothing heard yet"),
        };
        let r = &dev.node.prefs.radio;
        let last = match dev.history().iter().filter(|m| !m.outgoing).last() {
            Some(m) => {
                let text = String::from_utf8_lossy(&m.text);
                match m.peer {
                    // Channel texts already start with "Name: ".
                    mc_node::history::Peer::Channel(_) => String::from(text),
                    p => format!("{}: {text}", super::peer_name(ui, p)),
                }
            }
            None => String::new(),
        };
        alloc::vec![
            clock::date(clock::local(c.unix, c.tz_offset_min)),
            unread,
            last,
            heard,
            format!(
                "{} {} {}",
                name_of(&dev.node.prefs.name),
                ui.sym.dot,
                short_key(dev.node.id.pub_key())
            ),
            format!(
                "{}.{:03} MHz SF{} BW{}.{}",
                r.freq_khz / 1000,
                r.freq_khz % 1000,
                r.sf,
                r.bw_hz / 1000,
                r.bw_hz % 1000 / 100
            ),
        ]
    }
}

impl Screen for Home {
    fn title(&self, ui: &Ui<'_>) -> String {
        name_of(&ui.ctx.dev.node.prefs.name)
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let ch = Self::clock_rows(ui);
        // Text clock: what the terminal shows; on the panel, the overlay covers it.
        let time = clock::hhmm(clock::local(ui.ctx.unix, ui.ctx.tz_offset_min));
        f.render_widget(
            Paragraph::new(time)
                .alignment(Alignment::Center)
                .style(theme::label()),
            rows(body, ch / 2, 1),
        );
        let launcher_h = LAUNCHER.len() as u16;
        let room = body.height.saturating_sub(ch + launcher_h + 1);
        for (i, line) in Self::info(ui)
            .into_iter()
            .take(usize::from(room))
            .enumerate()
        {
            let align = if i == 0 {
                Alignment::Center
            } else {
                Alignment::Left
            };
            f.render_widget(
                Paragraph::new(line).alignment(align),
                rows(body, ch + i as u16, 1),
            );
        }
        let dev = ui.ctx.dev;
        let channels = dev.node.channels.0.iter().filter(|c| !c.is_empty()).count();
        let new = dev.unread_alerting();
        let launcher: Vec<(String, String)> = LAUNCHER
            .iter()
            .map(|(label, d)| {
                let detail = match d {
                    Dest::Messages if new > 0 => format!("{new} new"),
                    Dest::Contacts => format!("{}", dev.node.contacts.list.len()),
                    Dest::Channels => format!("{channels}"),
                    _ => String::new(),
                };
                (String::from(*label), detail)
            })
            .collect();
        draw_list(
            f,
            rows(body, body.height.saturating_sub(launcher_h), launcher_h),
            &launcher,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        if step(&mut self.state, LAUNCHER.len(), input) {
            let (_, d) = LAUNCHER[self.state.selected().unwrap_or(0) % LAUNCHER.len()];
            return Nav::Push(open(d));
        }
        Nav::Stay
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: Some("next"),
            a_hold: Some("open"),
            b_click: None,
            // B only toggles the light here: `B☀`.
            b_hold: Some("☀"),
        }
    }

    fn header_time(&self) -> bool {
        false
    }

    fn paints(&self, body: Rect, ui: &Ui<'_>) -> Vec<(Rect, Paint)> {
        let t = clock::local(ui.ctx.unix, ui.ctx.tz_offset_min).map(|t| (t.hour, t.minute));
        alloc::vec![(rows(body, 0, Self::clock_rows(ui)), Paint::Clock(t))]
    }
}
