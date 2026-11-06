//! `mc ui`: the device's interface in a terminal (`docs/spec/ui.md` §11). The real `mc-ui` App
//! runs on a simulated device (`mc-sim`) with two peers, Alice and Bob, who advertise
//! themselves. `--png <dir>` instead renders the tour (`mc_ui::tour`) through mousefood, the
//! way the panel shows it, overlays included.

use std::io::stdout;
use std::path::Path;
use std::time::{Duration as StdDuration, Instant as StdInstant};

use mc_core::{Duration, Instant};
use mc_proto::Command;
use mc_sim::World;
use mc_ui::app::{App, Ctx, Intent};
use mc_ui::input::{Button, Gesture};
use mc_ui::panel::{self, COMPACT, Frame, Grid, LARGE};
use ratatui_core::layout::Rect;
use ratatui_core::terminal::Terminal;
use ratatui_core::text::Line;
use ratatui_crossterm::CrosstermBackend;
use ratatui_crossterm::crossterm::event::{
    self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers,
};
use ratatui_crossterm::crossterm::execute;
use ratatui_crossterm::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui_widgets::block::Block;
use ratatui_widgets::paragraph::Paragraph;

use crate::port::now_unix;

const ME: usize = 0;

/// The device ("oxymesh") and two neighbours.
fn world(clock: u32) -> World {
    let mut w = World::new(7);
    let me = w.add("oxymesh", |_| {});
    for name in ["Alice", "Bob"] {
        let i = w.add(name, |_| {});
        w.link(me, i);
    }
    let mut frame = [0u8; 8];
    if let Ok(n) = (Command::SetDeviceTime { unix_s: clock }).encode(&mut frame) {
        w.app(ME, &frame[..n]);
    }
    w
}

fn ms(t: Instant) -> u64 {
    t.saturating_duration_since(Instant::ZERO).as_millis()
}

fn ctx(w: &World, light: bool) -> Ctx<'_> {
    let dev = &w.nodes[ME].dev;
    Ctx {
        dev,
        now_ms: ms(w.now),
        unix: dev.node.rtc.now(w.now),
        tz_offset_min: local_offset_min(),
        battery_percent: Some(87),
        charging: false,
        light,
    }
}

/// The Mac's UTC offset, from `date +%z` (no time zone crate for one number).
fn local_offset_min() -> i16 {
    use std::sync::OnceLock;
    static OFF: OnceLock<i16> = OnceLock::new();
    *OFF.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output();
        let s = out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let (sign, digits) = match s.split_at_checked(1) {
            Some(("-", d)) => (-1, d),
            Some(("+", d)) => (1, d),
            _ => return 0,
        };
        let h: i16 = digits.get(..2).and_then(|x| x.parse().ok()).unwrap_or(0);
        let m: i16 = digits.get(2..4).and_then(|x| x.parse().ok()).unwrap_or(0);
        sign * (h * 60 + m)
    })
}

pub fn run(compact: bool, png: Option<&Path>) -> Result<(), String> {
    let grid = if compact { COMPACT } else { LARGE };
    match png {
        Some(dir) => render_tour(dir),
        None => interactive(grid).map_err(|e| e.to_string()),
    }
}

/// Every tour screen at both grid sizes, as the panel shows them (×3).
fn render_tour(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut w = world(1_793_973_720);
    for (i, peer) in [(1, 1u64), (2, 4)] {
        w.run(Duration::from_secs(peer));
        let now = w.now;
        let _ = w.node(i).send_self_advert(now, false);
    }
    w.run(Duration::from_secs(3));
    let c = ctx(&w, false);
    for grid in [LARGE, COMPACT] {
        for (name, gestures) in mc_ui::tour::TOUR {
            let mut app = App::new(grid, true);
            for e in &w.nodes[ME].ui {
                app.event(e, &c);
            }
            for g in *gestures {
                app.gesture(*g, &c);
            }
            let mut frame = Frame::new();
            {
                let mut t = Terminal::new(panel::backend(&mut frame, grid))
                    .map_err(|e| format!("{e:?}"))?;
                t.draw(|f| app.draw(f, &c)).map_err(|e| format!("{e:?}"))?;
                for o in app.overlays() {
                    let _ = mc_ui::overlay::draw(o, t.backend_mut().display_mut());
                }
            }
            let size = if grid == LARGE { "large" } else { "compact" };
            let path = dir.join(format!("{name}_{size}.png"));
            crate::png::write_frame(&path, &frame, 3).map_err(|e| e.to_string())?;
            println!("{}", path.display());
        }
    }
    Ok(())
}

const HELP: [&str; 10] = [
    "j ↓   A click: next",
    "k ↑   A double: previous",
    "Enter A long: open",
    "Space A hold (toggle)",
    "Esc ⌫ B click: back",
    "h     B double: Home",
    "l     B long: light",
    "g     text size",
    "q     quit",
    "",
];

fn interactive(mut grid: Grid) -> std::io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    // Restore the terminal whatever happens, panics included.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        hook(info);
    }));
    let res = emulate(&mut grid);
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    res
}

fn emulate(grid: &mut Grid) -> std::io::Result<()> {
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut w = world(now_unix());
    let mut app = App::new(*grid, false);
    let start = StdInstant::now();
    let mut light = false;
    let mut hold_since: Option<StdInstant> = None;
    let mut ticks_sent = 0u32;
    let mut log: Vec<String> = Vec::new();
    // Alice and Bob advertise themselves a few seconds in, then every 2 minutes.
    let mut next_advert = [(1usize, 3_000u64), (2, 7_000)];
    loop {
        // The simulated world follows the wall clock.
        let wall = start.elapsed().as_millis() as u64;
        while ms(w.now) < wall {
            w.step(Duration::from_millis(5));
        }
        for (peer, at) in &mut next_advert {
            if wall >= *at {
                let now = w.now;
                let _ = w.node(*peer).send_self_advert(now, false);
                *at += 120_000;
            }
        }
        let events = std::mem::take(&mut w.nodes[ME].ui);
        {
            let c = ctx(&w, light);
            for e in &events {
                app.event(e, &c);
            }
            app.tick(c.now_ms);
        }
        // A held with Space: ticks every 250 ms from 1 s.
        let mut gestures: Vec<Gesture> = Vec::new();
        if let Some(t0) = hold_since {
            let held = t0.elapsed().as_millis() as u32;
            if held >= 1000 {
                let due = (held - 1000) / 250 + 1;
                if due > ticks_sent {
                    ticks_sent = due;
                    gestures.push(Gesture::Held(Button::A, held));
                }
            }
        }
        if event::poll(StdDuration::from_millis(40))?
            && let TermEvent::Key(k) = event::read()?
            && k.kind == KeyEventKind::Press
        {
            let g = match k.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                KeyCode::Char('j') | KeyCode::Down => Some(Gesture::Click(Button::A)),
                KeyCode::Char('k') | KeyCode::Up => Some(Gesture::Double(Button::A)),
                KeyCode::Enter => Some(Gesture::Long(Button::A)),
                KeyCode::Esc | KeyCode::Backspace => Some(Gesture::Click(Button::B)),
                KeyCode::Char('h') => Some(Gesture::Double(Button::B)),
                KeyCode::Char('l') => Some(Gesture::Long(Button::B)),
                KeyCode::Char(' ') => {
                    if hold_since.take().is_some() {
                        // Released: before 1 s it was a long press.
                        Some(if ticks_sent == 0 {
                            Gesture::Long(Button::A)
                        } else {
                            Gesture::HeldEnd(Button::A)
                        })
                    } else {
                        hold_since = Some(StdInstant::now());
                        ticks_sent = 0;
                        None
                    }
                }
                KeyCode::Char('g') => {
                    *grid = if *grid == LARGE { COMPACT } else { LARGE };
                    app.set_grid(*grid);
                    term.clear()?;
                    None
                }
                _ => None,
            };
            gestures.extend(g);
        }
        for g in gestures {
            let intent = {
                let c = ctx(&w, light);
                app.gesture(g, &c)
            };
            log.push(format!("{g:?}"));
            if let Some(i) = intent {
                log.push(format!("  → {i:?}"));
                match i {
                    Intent::ToggleLight => light = !light,
                    Intent::SendAdvert { flood } => {
                        let now = w.now;
                        let _ = w.node(ME).send_self_advert(now, flood);
                    }
                    Intent::Reboot | Intent::FactoryReset => {
                        app.toast("Ignored by the emulator", ms(w.now));
                    }
                }
            }
        }
        if log.len() > 12 {
            log.drain(..log.len() - 12);
        }
        let c = ctx(&w, light);
        term.draw(|f| {
            let area = f.area();
            let dev = Rect {
                x: area.x + 1,
                y: area.y + 1,
                width: grid.cols.min(area.width.saturating_sub(2)),
                height: grid.rows.min(area.height.saturating_sub(2)),
            };
            let frame = Rect {
                x: dev.x - 1,
                y: dev.y - 1,
                width: dev.width + 2,
                height: dev.height + 2,
            };
            f.render_widget(
                Block::bordered().title(format!(" M5 {}×{} ", grid.cols, grid.rows)),
                frame,
            );
            app.draw_in(f, dev, &c);
            let side = Rect {
                x: frame.right() + 2,
                y: area.y,
                width: area.width.saturating_sub(frame.right() + 2),
                height: area.height,
            };
            let mut lines: Vec<Line<'_>> = HELP.iter().map(|h| Line::from(*h)).collect();
            lines.push(Line::from(if hold_since.is_some() {
                "A is held (Space releases)"
            } else {
                ""
            }));
            lines.push(Line::from(""));
            lines.extend(log.iter().map(|l| Line::from(l.as_str())));
            f.render_widget(Paragraph::new(lines), side);
        })?;
    }
}
