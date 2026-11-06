//! Screens that show information: placeholders, Messages (empty until U2), About, My card.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::{Alignment, Rect};
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use super::{name_of, rows, short_key};
use crate::app::{Paint, Screen, Ui};
use crate::chrome::{self, Footer};

/// A feature not built yet: says which milestone brings it.
#[derive(Debug)]
pub struct Soon {
    title: String,
    note: String,
}

impl Soon {
    pub fn new(title: &str, note: &str) -> Self {
        Self {
            title: String::from(title),
            note: String::from(note),
        }
    }
}

impl Screen for Soon {
    fn title(&self, _ui: &Ui<'_>) -> String {
        self.title.clone()
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, _ui: &Ui<'_>) {
        f.render_widget(
            Paragraph::new(self.note.as_str()).wrap(Wrap { trim: true }),
            rows(body, 1, body.height.saturating_sub(1)),
        );
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }
}

/// Conversations: the local history arrives in U2, the screens in U3.
#[derive(Debug)]
pub struct Messages;

impl Screen for Messages {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("Messages")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let h = ui.ctx.dev.history();
        let n = h.iter().count();
        let text = match (n, h.unread()) {
            (0, _) => String::from("No messages yet."),
            (n, 0) => format!("{n} messages, all read."),
            (n, u) => format!("{n} messages, {u} unread."),
        };
        let lines = alloc::vec![
            Line::from(text),
            Line::from(""),
            Line::from("Conversations on the device arrive in U3."),
        ];
        f.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: true }),
            rows(body, 1, body.height.saturating_sub(1)),
        );
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }
}

/// System → About.
#[derive(Debug)]
pub struct About;

impl Screen for About {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("About")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let c = ui.ctx;
        let dev = c.dev;
        let p = &dev.node.prefs;
        let r = &p.radio;
        let w = usize::from(body.width);
        let channels = dev.node.channels.0.iter().filter(|c| !c.is_empty()).count();
        let up = c.now_ms / 1000;
        let volts = if dev.battery_mv == 0 {
            String::from("--")
        } else {
            format!(
                "{}.{:02} V",
                dev.battery_mv / 1000,
                dev.battery_mv % 1000 / 10
            )
        };
        let battery = match c.battery_percent {
            Some(pc) => format!("{volts} {pc}%"),
            None => volts,
        };
        let pairs: Vec<(&str, String)> = alloc::vec![
            ("Name", name_of(&p.name)),
            ("Key", short_key(dev.node.id.pub_key())),
            ("Firmware", String::from(dev.info.version)),
            (
                "Frequency",
                format!("{}.{:03} MHz", r.freq_khz / 1000, r.freq_khz % 1000)
            ),
            (
                "Modulation",
                format!(
                    "SF{} BW{}.{} CR4/{}",
                    r.sf,
                    r.bw_hz / 1000,
                    r.bw_hz % 1000 / 100,
                    r.cr
                )
            ),
            ("Power", format!("{} dBm", r.tx_power_dbm)),
            (
                "Contacts",
                format!(
                    "{}/{}",
                    dev.node.contacts.list.len(),
                    mc_node::contacts::MAX_CONTACTS
                )
            ),
            (
                "Channels",
                format!("{channels}/{}", mc_node::contacts::MAX_CHANNELS)
            ),
            ("Battery", battery),
            ("Uptime", format!("{} h {:02} min", up / 3600, up / 60 % 60)),
            ("For the app", format!("{} queued", dev.queued_messages())),
        ];
        let lines: Vec<Line<'_>> = pairs
            .iter()
            .map(|(k, v)| Line::from(chrome::spread(k, v, w)))
            .collect();
        f.render_widget(Paragraph::new(lines), body);
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }
}

/// Identity → My card: a QR the official app scans to add us.
#[derive(Debug)]
pub struct Card;

/// `meshcore://contact/add?name=…&public_key=…&type=1`, the official app's contact link.
pub fn contact_link(name: &str, key: &[u8; 32]) -> String {
    let mut n = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            n.push(char::from(b));
        } else {
            n.push_str(&format!("%{b:02X}"));
        }
    }
    format!(
        "meshcore://contact/add?name={n}&public_key={}&type=1",
        mc_core::Hex(key)
    )
}

impl Screen for Card {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("My card")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let dev = ui.ctx.dev;
        let qr = rows(body, 0, body.height.saturating_sub(1));
        // Text fallback under the QR (the terminal cannot show it).
        f.render_widget(
            Paragraph::new(alloc::vec![
                Line::from(""),
                Line::from("QR code shown"),
                Line::from("on the device:"),
                Line::from("scan it with the"),
                Line::from("MeshCore app."),
            ])
            .alignment(Alignment::Center),
            qr,
        );
        f.render_widget(
            Paragraph::new(format!(
                "{} {} {}",
                name_of(&dev.node.prefs.name),
                ui.sym.dot,
                short_key(dev.node.id.pub_key())
            ))
            .alignment(Alignment::Center),
            rows(body, body.height.saturating_sub(1), 1),
        );
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::BACK
    }

    fn paints(&self, body: Rect, ui: &Ui<'_>) -> Vec<(Rect, Paint)> {
        let dev = ui.ctx.dev;
        let link = contact_link(&name_of(&dev.node.prefs.name), dev.node.id.pub_key());
        alloc::vec![(
            rows(body, 0, body.height.saturating_sub(1)),
            Paint::Qr(link)
        )]
    }
}
