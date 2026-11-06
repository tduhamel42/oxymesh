//! The interface as a state machine: a stack of screens, a modal popup, a toast, the new
//! message banner. Fed with gestures and device notifications, it draws into a Ratatui frame
//! and queues `Intent`s; the runner applies them with [`crate::act::perform`] and reports the
//! outcome back. The App never touches the radio, the storage or the backlight itself.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mc_node::Device;
use mc_node::device::UiEvent;
use mc_node::history::Peer;
use ratatui_core::buffer::CellDiffOption;
use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_core::text::Line;
use ratatui_widgets::block::Block;
use ratatui_widgets::clear::Clear;
use ratatui_widgets::paragraph::{Paragraph, Wrap};

use crate::act::{Intent, Outcome};
use crate::chrome::{self, Footer};
use crate::input::{Button, Gesture};
use crate::keyboard::Keyboard;
use crate::overlay::{Overlay, PixelRect};
use crate::panel::{COMPACT, Grid};
use crate::screens::{self, Home};
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
    /// The GPS, when it has a fix.
    pub gps: Option<GpsFix>,
}

/// A GPS position fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpsFix {
    /// Degrees × 1e6.
    pub lat: i32,
    pub lon: i32,
    pub satellites: u8,
}

/// Context handed to screens.
#[derive(Debug, Clone, Copy)]
pub struct Ui<'a> {
    pub ctx: &'a Ctx<'a>,
    pub sym: &'static Symbols,
    pub grid: Grid,
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
    /// Open a screen and run an intent (open a conversation and mark it read).
    PushWith(Box<dyn Screen>, Intent),
    /// Replace the current screen (steps of a flow).
    Replace(Box<dyn Screen>),
    Pop,
    /// Run the intent, show the toast if any, and go back.
    PopWith(Intent, Option<&'static str>),
    /// Run the intent and go back to Home.
    HomeWith(Intent),
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
    /// Map points: label and position in thousandths of the half-width (east, north); we are
    /// at the center.
    Map(Vec<(String, i16, i16)>),
}

pub trait Screen: core::fmt::Debug {
    fn title(&self, ui: &Ui<'_>) -> String;
    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>);
    fn input(&mut self, _input: Input, _ui: &Ui<'_>) -> Nav {
        Nav::Stay
    }
    /// B click: `None` goes back (the default); a screen may use B itself (deleting text).
    fn back(&mut self, _ui: &Ui<'_>) -> Option<Nav> {
        None
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
    /// The conversation shown, if any (no banner for it; new messages are read at once).
    fn peer(&self) -> Option<Peer> {
        None
    }
    /// The outcome of an intent this screen asked for.
    fn result(&mut self, _intent: &Intent, _outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        Nav::Stay
    }
    /// The text entry, for tests and scripts.
    fn keyboard(&self) -> Option<&Keyboard> {
        None
    }
    /// Called regularly while the screen is shown (the range test sends its pings here).
    fn tick(&mut self, _ui: &Ui<'_>) -> Nav {
        Nav::Stay
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
    /// A received message not seen yet: its history entry.
    banner: Option<u32>,
    intents: Vec<Intent>,
    grid: Grid,
    /// Drawing for the panel: overlays are painted, their cells skipped by Ratatui.
    panel: bool,
    overlays: Vec<Overlay>,
    dirty: bool,
    screen_changed: bool,
}

impl App {
    pub fn new(grid: Grid, panel: bool) -> Self {
        Self {
            stack: alloc::vec![Box::new(Home::default()) as Box<dyn Screen>],
            popup: None,
            toast: None,
            banner: None,
            intents: Vec::new(),
            grid,
            panel,
            overlays: Vec::new(),
            dirty: true,
            screen_changed: true,
        }
    }

    /// At boot: the first-boot setup, unless it was done or skipped.
    pub fn start(&mut self, ctx: &Ctx<'_>) {
        if !ctx.dev.node.prefs.ui.setup_done {
            self.push(Box::new(screens::setup_start()));
        }
    }

    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// Follows the text size setting; true if the grid changed (the runner then rebuilds its
    /// terminal for [`App::grid`]).
    pub fn sync(&mut self, ctx: &Ctx<'_>) -> bool {
        let want = if ctx.dev.node.prefs.ui.compact {
            COMPACT
        } else {
            crate::panel::LARGE
        };
        if want != self.grid {
            self.set_grid(want);
            return true;
        }
        false
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

    pub fn banner(&self) -> Option<u32> {
        self.banner
    }

    /// The text entry on screen, if any.
    pub fn keyboard(&self) -> Option<&Keyboard> {
        self.stack.last().and_then(|s| s.keyboard())
    }

    /// Something changed since the last draw.
    pub fn take_dirty(&mut self) -> bool {
        core::mem::take(&mut self.dirty)
    }

    /// The screen changed since the last call (refresh policy).
    pub fn take_screen_changed(&mut self) -> bool {
        core::mem::take(&mut self.screen_changed)
    }

    /// What the runner must apply now ([`crate::act::perform`]), in order.
    pub fn take_intents(&mut self) -> Vec<Intent> {
        core::mem::take(&mut self.intents)
    }

    /// The runner applied `intent`: the screen that asked may react.
    pub fn done(&mut self, intent: &Intent, outcome: &Outcome, ctx: &Ctx<'_>) {
        self.dirty = true;
        if let Outcome::Failed(_) = outcome {
            self.toast("Failed", ctx.now_ms);
        }
        let ui = make_ui(self.grid, ctx);
        let nav = match self.stack.last_mut() {
            Some(s) => s.result(intent, outcome, &ui),
            None => Nav::Stay,
        };
        self.apply(nav, ctx);
    }

    pub fn toast(&mut self, text: &str, now_ms: u64) {
        self.toast = Some((String::from(text), now_ms + TOAST_MS));
        self.dirty = true;
    }

    /// Timers: toasts expire, the screen on top may act. Call a few times a second.
    pub fn tick(&mut self, ctx: &Ctx<'_>) {
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, until)| ctx.now_ms >= *until)
        {
            self.toast = None;
            self.dirty = true;
        }
        let ui = make_ui(self.grid, ctx);
        let nav = match self.stack.last_mut() {
            Some(s) => s.tick(&ui),
            None => Nav::Stay,
        };
        if !matches!(nav, Nav::Stay) {
            self.dirty = true;
            self.apply(nav, ctx);
        }
    }

    /// A notification from the device (`Device::poll_ui`).
    pub fn event(&mut self, e: &UiEvent, _ctx: &Ctx<'_>) {
        self.dirty = true;
        if let UiEvent::Message {
            seq,
            peer,
            outgoing: false,
        } = *e
        {
            let shown = self.stack.last().and_then(|s| s.peer());
            if shown == Some(peer) {
                // Already on screen: read.
                self.intents.push(Intent::MarkRead(peer));
            } else if matches!(peer, Peer::Contact(_)) {
                // Banner for direct messages only; channels just count as unread.
                self.banner = Some(seq);
            }
        }
    }

    /// A gesture.
    pub fn gesture(&mut self, g: Gesture, ctx: &Ctx<'_>) {
        self.dirty = true;
        if g == Gesture::Long(Button::B) {
            self.intents.push(Intent::ToggleLight);
            return;
        }
        if let Some(seq) = self.banner {
            // The first press acts on the banner and is never lost.
            match g {
                Gesture::Click(Button::A)
                | Gesture::Double(Button::A)
                | Gesture::Long(Button::A) => {
                    self.banner = None;
                    if let Some(m) = ctx.dev.history().get(seq) {
                        let peer = m.peer;
                        self.apply(
                            Nav::PushWith(
                                Box::new(screens::Conversation::new(peer)),
                                Intent::MarkRead(peer),
                            ),
                            ctx,
                        );
                    }
                    return;
                }
                Gesture::Click(Button::B) | Gesture::Double(Button::B) => {
                    self.banner = None;
                    return;
                }
                _ => {}
            }
        }
        match g {
            Gesture::Double(Button::B) => {
                self.popup = None;
                if self.stack.len() > 1 {
                    self.stack.truncate(1);
                    self.screen_changed = true;
                }
                return;
            }
            Gesture::Click(Button::B) => {
                if self.popup.take().is_some() {
                    return;
                }
                let ui = make_ui(self.grid, ctx);
                let own = self.stack.last_mut().and_then(|s| s.back(&ui));
                match own {
                    Some(nav) => self.apply(nav, ctx),
                    None => {
                        if self.stack.len() > 1 {
                            self.stack.pop();
                            self.screen_changed = true;
                        }
                    }
                }
                return;
            }
            _ => {}
        }
        if let Some(p) = self.popup.as_mut() {
            match g {
                Gesture::Held(Button::A, ms) => {
                    p.held_ms = ms;
                    if ms >= CONFIRM_MS {
                        let (intent, toast) = (p.intent.clone(), p.toast);
                        self.popup = None;
                        if let Some(t) = toast {
                            self.toast(t, ctx.now_ms);
                        }
                        self.intents.push(intent);
                    }
                }
                Gesture::HeldEnd(Button::A) => p.held_ms = 0,
                _ => {}
            }
            return;
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
            _ => return,
        };
        let ui = make_ui(self.grid, ctx);
        let nav = match self.stack.last_mut() {
            Some(s) => s.input(input, &ui),
            None => Nav::Stay,
        };
        self.apply(nav, ctx);
    }

    fn apply(&mut self, nav: Nav, ctx: &Ctx<'_>) {
        match nav {
            Nav::Stay => {}
            Nav::Push(s) => self.push(s),
            Nav::PushWith(s, i) => {
                self.push(s);
                self.intents.push(i);
            }
            Nav::Replace(s) => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                }
                self.push(s);
            }
            Nav::Pop => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                    self.screen_changed = true;
                }
            }
            Nav::PopWith(i, toast) => {
                self.intents.push(i);
                if let Some(t) = toast {
                    self.toast(t, ctx.now_ms);
                }
                if self.stack.len() > 1 {
                    self.stack.pop();
                    self.screen_changed = true;
                }
            }
            Nav::HomeWith(i) => {
                self.intents.push(i);
                self.stack.truncate(1);
                self.screen_changed = true;
            }
            Nav::Intent(i) => self.intents.push(i),
            Nav::IntentToast(i, t) => {
                self.toast(t, ctx.now_ms);
                self.intents.push(i);
            }
            Nav::Confirm(c) => self.popup = Some(c),
            Nav::Toast(t) => self.toast(&t, ctx.now_ms),
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
        let ui = make_ui(self.grid, ctx);
        let areas = chrome::areas(area);
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        let title = top.title(&ui);
        chrome::header(f, areas.header, &title, top.header_time(), &ui);
        top.draw(f, areas.body, &ui);
        self.overlays.clear();
        let covered = self.popup.is_some() || self.banner.is_some();
        if self.panel && !covered {
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
                    Paint::Map(points) => Overlay::Map { at, points },
                });
            }
        }
        let hints = if self.popup.is_some() {
            Footer {
                a_click: None,
                a_hold: Some("confirm"),
                b_click: Some("cancel"),
                b_hold: None,
            }
        } else if self.banner.is_some() {
            Footer {
                a_click: None,
                a_hold: Some("open"),
                b_click: Some("later"),
                b_hold: None,
            }
        } else {
            top.footer(&ui)
        };
        let toast = self.toast.as_ref().map(|(t, _)| t.as_str());
        chrome::footer(f, areas.footer, &hints, toast, &ui);
        if let Some(p) = &self.popup {
            draw_confirm(f, areas.body, p, &ui);
        } else if let Some(seq) = self.banner {
            draw_banner(f, areas.body, seq, &ui);
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

fn make_ui<'a>(grid: Grid, ctx: &'a Ctx<'a>) -> Ui<'a> {
    Ui {
        ctx,
        sym: theme::symbols(grid),
        grid,
    }
}

/// Size of a cell in pixels.
pub fn cell_px(grid: Grid) -> (u32, u32) {
    if grid == COMPACT { (6, 10) } else { (8, 13) }
}

/// New direct message: sender and the start of the text, over the top of the body.
fn draw_banner(f: &mut Frame<'_>, body: Rect, seq: u32, ui: &Ui<'_>) {
    let Some(m) = ui.ctx.dev.history().get(seq) else {
        return;
    };
    let w = usize::from(body.width);
    let area = Rect {
        height: 4.min(body.height),
        ..body
    };
    f.render_widget(Clear, area);
    let who = screens::peer_name(ui, m.peer);
    let text = String::from_utf8_lossy(&m.text);
    let mut lines = alloc::vec![Line::from(chrome::fit(&format!("New message: {who}"), w))];
    for l in chrome::wrap(&text, w)
        .into_iter()
        .take(usize::from(area.height) - 1)
    {
        lines.push(Line::from(l));
    }
    f.render_widget(Paragraph::new(lines).style(theme::reversed()), area);
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
