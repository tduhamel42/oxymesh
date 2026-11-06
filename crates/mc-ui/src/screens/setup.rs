//! First-boot setup (`docs/spec/ui.md` §6): region, name, time. Skippable (B); it comes back
//! from Settings → System → Setup again.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::list::ListState;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use super::chat::{Compose, Target};
use super::{draw_list, name_of, rows, step};
use crate::act::{Intent, Outcome};
use crate::app::{Confirm, Input, Nav, Screen, Ui};
use crate::chrome::Footer;
use crate::clock;
use crate::regions::{Region, presets};

/// Step 1: the region preset.
#[derive(Debug)]
pub struct SetupRegion {
    state: ListState,
    regions: Vec<Region>,
}

impl Default for SetupRegion {
    fn default() -> Self {
        Self {
            state: ListState::default().with_selected(Some(0)),
            regions: presets(),
        }
    }
}

/// Short form for a list row: `869.6 SF8`.
fn describe(freq_khz: u32, sf: u8) -> String {
    format!("{}.{} SF{sf}", freq_khz / 1000, freq_khz % 1000 / 100)
}

/// Long form for the confirmation: `869.618 MHz, SF8, 62.5 kHz`.
fn describe_long(freq_khz: u32, bw_hz: u32, sf: u8) -> String {
    format!(
        "{}.{:03} MHz, SF{sf}, {}.{} kHz",
        freq_khz / 1000,
        freq_khz % 1000,
        bw_hz / 1000,
        bw_hz % 1000 / 100
    )
}

fn name_step(ui: &Ui<'_>) -> Box<dyn Screen> {
    Box::new(Compose::new(
        Target::Name { setup: true },
        &name_of(&ui.ctx.dev.node.prefs.name),
    ))
}

impl Screen for SetupRegion {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("1/3 Region")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let r = &ui.ctx.dev.node.prefs.radio;
        let mut items: Vec<(String, String)> =
            alloc::vec![(String::from("Keep current"), describe(r.freq_khz, r.sf))];
        items.extend(
            self.regions
                .iter()
                .map(|g| (g.name.clone(), describe(g.freq_khz, g.sf))),
        );
        f.render_widget(
            Paragraph::new("Radio settings shared by the nodes around you.")
                .wrap(Wrap { trim: true }),
            rows(body, 0, 2),
        );
        draw_list(
            f,
            rows(body, 3, body.height.saturating_sub(3)),
            &items,
            &mut self.state,
            ui,
        );
    }

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        if !step(&mut self.state, self.regions.len() + 1, input) {
            return Nav::Stay;
        }
        match self.state.selected().unwrap_or(0) {
            0 => Nav::Replace(name_step(ui)),
            i => match self.regions.get(i - 1) {
                Some(g) => Nav::Confirm(Confirm::new(
                    "Region",
                    &format!(
                        "Switch the radio to {}: {}.",
                        g.name,
                        describe_long(g.freq_khz, g.bw_hz, g.sf)
                    ),
                    Intent::SetRadio {
                        freq_khz: g.freq_khz,
                        bw_hz: g.bw_hz,
                        sf: g.sf,
                        cr: g.cr,
                    },
                )),
                None => Nav::Stay,
            },
        }
    }

    fn back(&mut self, _ui: &Ui<'_>) -> Option<Nav> {
        Some(Nav::HomeWith(Intent::SetupDone))
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: Some("next"),
            a_hold: Some("choose"),
            b_click: Some("skip"),
            b_hold: None,
        }
    }

    fn result(&mut self, intent: &Intent, outcome: &Outcome, ui: &Ui<'_>) -> Nav {
        if matches!(intent, Intent::SetRadio { .. }) && *outcome == Outcome::Ok {
            return Nav::Replace(name_step(ui));
        }
        Nav::Stay
    }
}

/// Step 3: the time comes from the phone app or the GPS.
#[derive(Debug)]
pub struct SetupTime;

impl Screen for SetupTime {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from("3/3 Time")
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let c = ui.ctx;
        let t = clock::local(c.unix, c.tz_offset_min);
        let state = match t {
            Some(_) => format!("Clock: {} {}", clock::hhmm(t), clock::date(t)),
            None => String::from("Clock not set yet."),
        };
        let lines = alloc::vec![
            Line::from(state),
            Line::from(""),
            Line::from(
                "The phone app sets the time when it connects; the GPS does too once it has a fix."
            ),
            Line::from(""),
            Line::from("Hold A to finish."),
        ];
        f.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: true }),
            rows(body, 1, body.height.saturating_sub(1)),
        );
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        match input {
            Input::Open => Nav::HomeWith(Intent::SetupDone),
            _ => Nav::Stay,
        }
    }

    fn back(&mut self, _ui: &Ui<'_>) -> Option<Nav> {
        Some(Nav::HomeWith(Intent::SetupDone))
    }

    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer {
            a_click: None,
            a_hold: Some("finish"),
            b_click: Some("skip"),
            b_hold: None,
        }
    }
}
