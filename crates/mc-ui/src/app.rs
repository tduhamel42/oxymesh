//! The interface as a state machine: a stack of screens, a modal popup, a toast. Fed with
//! gestures and node events, it draws into a Ratatui frame and tells the runner what to do
//! (`Intent`). It never touches the radio, the storage or the backlight itself.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mc_node::{Device, Event};
use ratatui_core::buffer::CellDiffOption;
use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::block::Block;
use ratatui_widgets::clear::Clear;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use crate::chrome::{self, Footer};
use crate::input::{Button, Gesture};
use crate::overlay::{Overlay, PixelRect};
use crate::panel::{COMPACT, Grid};
use crate::screens::Home;
use crate::theme::{self, Symbols};

/// What the runner provides for one draw or one input.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    pub dev: &'a Device,
    /// Monotonic time.
    pub now_ms: u64,
    /// Device clock, Unix seconds (below 2020: not set).
    pub unix: u32,
    pub tz_offset_min: i16,
    pub battery_percent: Option<u8>,
    pub charging: bool,
    pub light: bool,
}

/// Context handed to screens.
#[derive(Debug, Clone, Copy)]
pub struct Ui<'a> {
    pub ctx: &'a Ctx<'a>,
    pub sym: &'static Symbols,
    pub grid: Grid,
    /// Last advert heard: name and time.
    pub heard: Option<(&'a str, u32)>,
}

/// Something only the runner can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    ToggleLight,
    SendAdvert { flood: bool },
    Reboot,
    FactoryReset,
}

/// A-button input as screens see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Down by this many rows (more while A is held).
    Next(u16),
    Prev,
    Open,
}

/// A screen's answer to an input.
#[derive(Debug)]
pub enum Nav {
    Stay,
    Push(Box<dyn Screen>),
    Pop,
    Intent(Intent),
    /// Run the intent and show the toast.
    IntentToast(Intent, &'static str),
    Confirm(Confirm),
    Toast(String),
}

/// A part of the screen drawn in pixels (`overlay`), in cells of the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Paint {
    Clock(Option<(u8, u8)>),
    Qr(String),
}

pub trait Screen: core::fmt::Debug {
    fn title(&self, ui: &Ui<'_>) -> String;
    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>);
    fn input(&mut self, _input: Input, _ui: &Ui<'_>) -> Nav {
        Nav::Stay
    }
    fn footer(&self, _ui: &Ui<'_>) -> Footer {
        Footer::LIST
    }
    /// Whether the header shows the time (Home has its big clock instead).
    fn header_time(&self) -> bool {
        true
    }
    /// Pixel parts of the screen, as cell rectangles inside `body`.
    fn paints(&self, _body: Rect, _ui: &Ui<'_>) -> Vec<(Rect, Paint)> {
        Vec::new()
    }
}

/// Hold-to-confirm popup (`docs/spec/ui.md` §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub title: String,
    pub text: String,
    pub intent: Intent,
    pub toast: Option<&'static str>,
    held_ms: u32,
}

impl Confirm {
    pub fn new(title: &str, text: &str, intent: Intent) -> Self {
        Self {
            title: String::from(title),
            text: String::from(text),
            intent,
            toast: None,
            held_ms: 0,
        }
    }

    pub fn with_toast(mut self, t: &'static str) -> Self {
        self.toast = Some(t);
        self
    }
}

pub const CONFIRM_MS: u32 = 2_000;
pub const TOAST_MS: u64 = 3_000;

#[derive(Debug)]
pub struct App {
    stack: Vec<Box<dyn Screen>>,
    popup: Option<Confirm>,
    toast: Option<(String, u64)>,
    grid: Grid,
    /// Drawing for the panel: overlays are painted, their cells skipped by Ratatui.
    panel: bool,
    overlays: Vec<Overlay>,
    dirty: bool,
    screen_changed: bool,
    last_heard: Option<(String, u32)>,
}

impl App {
    pub fn new(grid: Grid, panel: bool) -> Self {
        Self {
            stack: alloc::vec![Box::new(Home::default()) as Box<dyn Screen>],
            popup: None,
            toast: None,
            grid,
            panel,
            overlays: Vec::new(),
            dirty: true,
            screen_changed: true,
            last_heard: None,
        }
    }

    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// The runner rebuilds its terminal for the new grid.
    pub fn set_grid(&mut self, grid: Grid) {
        self.grid = grid;
        self.dirty = true;
        self.screen_changed = true;
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    pub fn popup(&self) -> Option<&Confirm> {
        self.popup.as_ref()
    }

    /// Something changed since the last draw.
    pub fn take_dirty(&mut self) -> bool {
        core::mem::take(&mut self.dirty)
    }

    /// The screen changed since the last call (refresh policy).
    pub fn take_screen_changed(&mut self) -> bool {
        core::mem::take(&mut self.screen_changed)
    }

    pub fn last_heard(&self) -> Option<(&str, u32)> {
        self.last_heard.as_ref().map(|(n, t)| (n.as_str(), *t))
    }

    pub fn toast(&mut self, text: &str, now_ms: u64) {
        self.toast = Some((String::from(text), now_ms + TOAST_MS));
        self.dirty = true;
    }

    /// Timers: toasts expire.
    pub fn tick(&mut self, now_ms: u64) {
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, until)| now_ms >= *until)
        {
            self.toast = None;
            self.dirty = true;
        }
    }

    /// A node event (`Device::poll_ui`).
    pub fn event(&mut self, e: &Event, ctx: &Ctx<'_>) {
        if let Event::AdvertHeard { key, .. } = e {
            let name = ctx
                .dev
                .node
                .contacts
                .find(key)
                .and_then(|s| ctx.dev.node.contacts.get(s))
                .map(|c| String::from_utf8_lossy(mc_node::contacts::name_str(&c.name)).into_owned())
                .unwrap_or_else(|| format!("{}", mc_core::Hex(&key[..4])));
            self.last_heard = Some((name, ctx.unix));
            self.dirty = true;
        }
    }

    /// A gesture; returns what the runner must do, if anything.
    pub fn gesture(&mut self, g: Gesture, ctx: &Ctx<'_>) -> Option<Intent> {
        self.dirty = true;
        match g {
            Gesture::Long(Button::B) => return Some(Intent::ToggleLight),
            Gesture::Double(Button::B) => {
                self.popup = None;
                if self.stack.len() > 1 {
                    self.stack.truncate(1);
                    self.screen_changed = true;
                }
                return None;
            }
            Gesture::Click(Button::B) => {
                if self.popup.take().is_none() && self.stack.len() > 1 {
                    self.stack.pop();
                    self.screen_changed = true;
                }
                return None;
            }
            _ => {}
        }
        if let Some(p) = self.popup.as_mut() {
            match g {
                Gesture::Held(Button::A, ms) => {
                    p.held_ms = ms;
                    if ms >= CONFIRM_MS {
                        let (intent, toast) = (p.intent, p.toast);
                        self.popup = None;
                        if let Some(t) = toast {
                            self.toast(t, ctx.now_ms);
                        }
                        return Some(intent);
                    }
                }
                Gesture::HeldEnd(Button::A) => p.held_ms = 0,
                _ => {}
            }
            return None;
        }
        let input = match g {
            Gesture::Click(Button::A) => Input::Next(1),
            Gesture::Double(Button::A) => Input::Prev,
            Gesture::Long(Button::A) => Input::Open,
            // Faster after 2 s and 4 s of holding.
            Gesture::Held(Button::A, ms) => Input::Next(match ms {
                0..2_000 => 1,
                2_000..4_000 => 2,
                _ => 4,
            }),
            _ => return None,
        };
        let ui = make_ui(self.grid, &self.last_heard, ctx);
        let nav = match self.stack.last_mut() {
            Some(s) => s.input(input, &ui),
            None => Nav::Stay,
        };
        self.apply(nav, ctx)
    }

    fn apply(&mut self, nav: Nav, ctx: &Ctx<'_>) -> Option<Intent> {
        match nav {
            Nav::Stay => None,
            Nav::Push(s) => {
                self.stack.push(s);
                self.screen_changed = true;
                None
            }
            Nav::Pop => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                    self.screen_changed = true;
                }
                None
            }
            Nav::Intent(i) => Some(i),
            Nav::IntentToast(i, t) => {
                self.toast(t, ctx.now_ms);
                Some(i)
            }
            Nav::Confirm(c) => {
                self.popup = Some(c);
                None
            }
            Nav::Toast(t) => {
                self.toast(&t, ctx.now_ms);
                None
            }
        }
    }

    /// Draws the whole grid. On the panel, the cells under overlays are marked skipped and the
    /// overlays are kept for [`App::overlays`].
    pub fn draw(&mut self, f: &mut Frame<'_>, ctx: &Ctx<'_>) {
        let area = f.area();
        self.draw_in(f, area, ctx);
    }

    /// Draws into `area` only (the emulator frames the device inside a larger terminal).
    /// Overlay pixels are relative to `area`'s top-left cell.
    pub fn draw_in(&mut self, f: &mut Frame<'_>, area: Rect, ctx: &Ctx<'_>) {
        let ui = make_ui(self.grid, &self.last_heard, ctx);
        let areas = chrome::areas(area);
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        let title = top.title(&ui);
        chrome::header(f, areas.header, &title, top.header_time(), &ui);
        top.draw(f, areas.body, &ui);
        self.overlays.clear();
        if self.panel && self.popup.is_none() {
            let (cw, ch) = cell_px(self.grid);
            for (cells, paint) in top.paints(areas.body, &ui) {
                let cells = cells.intersection(areas.body);
                for y in cells.top()..cells.bottom() {
                    for x in cells.left()..cells.right() {
                        f.buffer_mut()[(x, y)].set_diff_option(CellDiffOption::Skip);
                    }
                }
                let at = PixelRect {
                    x: i32::from(cells.x - area.x) * cw as i32,
                    y: i32::from(cells.y - area.y) * ch as i32,
                    w: u32::from(cells.width) * cw,
                    h: u32::from(cells.height) * ch,
                };
                self.overlays.push(match paint {
                    Paint::Clock(time) => Overlay::Clock { at, time },
                    Paint::Qr(text) => Overlay::Qr { at, text },
                });
            }
        }
        let hints = match &self.popup {
            Some(_) => Footer {
                a_click: None,
                a_hold: Some("confirm"),
                b_click: Some("cancel"),
                b_hold: None,
            },
            None => top.footer(&ui),
        };
        let toast = self.toast.as_ref().map(|(t, _)| t.as_str());
        chrome::footer(f, areas.footer, &hints, toast, &ui);
        if let Some(p) = &self.popup {
            draw_confirm(f, areas.body, p, &ui);
        }
    }

    /// Pixel overlays of the last draw, to paint over the panel frame.
    pub fn overlays(&self) -> &[Overlay] {
        &self.overlays
    }

    /// Opens a screen directly (tests, emulator scripts).
    pub fn push(&mut self, s: Box<dyn Screen>) {
        self.stack.push(s);
        self.screen_changed = true;
        self.dirty = true;
    }
}

fn make_ui<'a>(grid: Grid, heard: &'a Option<(String, u32)>, ctx: &'a Ctx<'a>) -> Ui<'a> {
    Ui {
        ctx,
        sym: theme::symbols(grid),
        grid,
        heard: heard.as_ref().map(|(n, t)| (n.as_str(), *t)),
    }
}

/// Size of a cell in pixels.
pub fn cell_px(grid: Grid) -> (u32, u32) {
    if grid == COMPACT { (6, 10) } else { (8, 13) }
}

fn draw_confirm(f: &mut Frame<'_>, body: Rect, p: &Confirm, ui: &Ui<'_>) {
    let inner_w = usize::from(body.width.saturating_sub(2)).max(1);
    // Borders, the wrapped text (one spare row for word wrapping), a blank, two lines.
    let text_rows = p.text.chars().count().div_ceil(inner_w) + 1;
    let h = u16::try_from(2 + text_rows + 3)
        .unwrap_or(u16::MAX)
        .min(body.height);
    let area = Rect {
        y: body.y + (body.height - h) / 2,
        height: h,
        ..body
    };
    f.render_widget(Clear, area);
    let left = CONFIRM_MS.saturating_sub(p.held_ms);
    let bar_len = inner_w.saturating_sub(8);
    let filled = bar_len * p.held_ms.min(CONFIRM_MS) as usize / CONFIRM_MS as usize;
    let progress = format!(
        "{}{} {}.{} s",
        "█".repeat(filled),
        ui.sym.dot.repeat(bar_len - filled),
        left / 1000,
        left % 1000 / 100
    );
    let block = Block::bordered().title(Line::from(chrome::fit(&p.title, inner_w)));
    let text = alloc::vec![
        Line::from(p.text.as_str()),
        Line::from(""),
        Line::from(String::from("Hold A to confirm")).style(theme::label()),
        Line::from(progress),
    ];
    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: true }).block(block),
        area,
    );
}
