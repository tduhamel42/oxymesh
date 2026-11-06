//! `mc ui`: the device's interface in a terminal (`docs/spec/ui.md` §11). The real `mc-ui` App
//! runs on a simulated device (`mc-sim`) with two peers who advertise themselves: Alice, an
//! echo bot who greets you, and Bob, who posts on the Public channel. `--png <dir>` instead renders the tour (`mc_ui::tour`) through mousefood, the
//! way the panel shows it, overlays included.

use std::io::stdout;
use std::path::Path;
use std::time::{Duration as StdDuration, Instant as StdInstant};

use mc_core::{Duration, Instant};
use mc_node::Event;
use mc_proto::Command;
use mc_sim::World;
use mc_ui::app::{App, Ctx, GpsFix};
use mc_ui::input::{Button, Gesture};
use mc_ui::panel::{self, COMPACT, Frame, Grid, LARGE};
use mc_ui::{Ble, BleState, Intent, Outcome, perform};
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

use rand_core::SeedableRng;

use crate::port::now_unix;

const ME: usize = 0;

/// The device ("oxymesh") and three neighbours: Alice (echo bot, with a position), Bob
/// (posts on Public, answers pings) and a relay that forwards and answers like a repeater
/// (admin password "secret"), with a position.
fn world(clock: u32) -> World {
    let mut w = World::new(7);
    let me = w.add("oxymesh", |_| {});
    let alice = w.add("Alice", |p| {
        p.advert_loc_policy = 1;
        p.lat = 45_930_000;
        p.lon = 6_900_000;
    });
    let bob = w.add("Bob", |p| p.telemetry_base = 2);
    let relay = w.add("Relay Aiguille", |p| {
        p.repeat = true;
        p.advert_loc_policy = 1;
        p.lat = 45_878_600;
        p.lon = 6_887_200;
    });
    for i in [alice, bob, relay] {
        w.link(me, i);
    }
    // The relay answers like a MeshCore repeater: discovery, login (admin password "secret",
    // guests without one), status, neighbours.
    w.serve_repeater(relay, "secret", "");
    w.link(relay, bob);
    let mut frame = [0u8; 8];
    if let Ok(n) = (Command::SetDeviceTime { unix_s: clock }).encode(&mut frame) {
        w.app(ME, &frame[..n]);
    }
    w
}

fn ms(t: Instant) -> u64 {
    t.saturating_duration_since(Instant::ZERO).as_millis()
}

/// A phone that has paired once and is in range: `p` makes it pair, connect, leave.
const PHONE: Ble = Ble {
    state: BleState::Advertising,
    pin: 482_913,
    paired: 1,
};

fn ctx(w: &World, light: bool, ble: Ble) -> Ctx<'_> {
    let dev = &w.nodes[ME].dev;
    Ctx {
        dev,
        now_ms: ms(w.now),
        unix: dev.node.rtc.now(w.now),
        tz_offset_min: dev.node.prefs.ui.tz_offset_min,
        battery_percent: Some(87),
        charging: false,
        light,
        ble,
        // A fixed fix near Chamonix, for the GPS and Map tools.
        gps: Some(GpsFix {
            lat: 45_923_700,
            lon: 6_869_400,
            satellites: 9,
        }),
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

/// Node `from` writes to the device.
fn say(w: &mut World, from: usize, text: &str) {
    let now = w.now;
    let ts = w.node(from).time(now);
    let to: [u8; 6] = w.nodes[ME].dev.node.id.pub_key()[..6]
        .try_into()
        .unwrap_or([0; 6]);
    let _ = w.node(from).send_text(now, &to, 0, 0, ts, text.as_bytes());
}

/// Node `from` writes on the Public channel.
fn say_channel(w: &mut World, from: usize, text: &str) {
    let now = w.now;
    let ts = w.node(from).time(now);
    let _ = w.node(from).send_channel_text(now, 0, ts, text.as_bytes());
}

/// Every tour screen at both grid sizes, as the panel shows them (×3).
fn render_tour(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut w = world(1_793_973_720);
    for i in [1, 2, 3, ME] {
        let now = w.now;
        let _ = w.node(i).send_self_advert(now, false);
        w.run(Duration::from_secs(3));
    }
    say_channel(&mut w, 1, "Snow on the pass, take chains");
    w.run(Duration::from_secs(5));
    say(&mut w, 2, "Coffee at the refuge around 4?");
    w.run(Duration::from_secs(5));
    let c = ctx(&w, false, PHONE);
    let pairing = ctx(
        &w,
        false,
        Ble {
            state: BleState::Pairing,
            ..PHONE
        },
    );
    let shots = mc_ui::tour::TOUR.iter().map(|(n, g)| (*n, *g, &c)).chain([(
        "ble_pairing",
        &[][..],
        &pairing,
    )]);
    for grid in [LARGE, COMPACT] {
        for (name, gestures, c) in shots.clone() {
            let mut app = App::new(grid, true);
            for g in gestures {
                app.gesture(*g, c);
            }
            let mut frame = Frame::new();
            {
                let mut t = Terminal::new(panel::backend(&mut frame, grid))
                    .map_err(|e| format!("{e:?}"))?;
                t.draw(|f| app.draw(f, c)).map_err(|e| format!("{e:?}"))?;
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

const HELP: [&str; 11] = [
    "j ↓   A click: next",
    "k ↑   A double: previous",
    "Enter A long: open",
    "Space A hold (toggle)",
    "Esc ⌫ B click: back",
    "h     B double: Home",
    "l     B long: light",
    "g     text size",
    "p     phone: pair, connect, leave",
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
    // The Mac's time zone, until changed in Settings → Time.
    w.nodes[ME].dev.node.prefs.ui.tz_offset_min = local_offset_min();
    w.nodes[ME].dev.node.prefs.ui.compact = *grid == COMPACT;
    let mut app = App::new(*grid, false);
    {
        let c = ctx(&w, false, PHONE);
        app.start(&c);
    }
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(1);
    let me_key = *w.nodes[ME].dev.node.id.pub_key();
    let start = StdInstant::now();
    let mut light = false;
    let mut ble = PHONE;
    let mut hold_since: Option<StdInstant> = None;
    let mut ticks_sent = 0u32;
    let mut log: Vec<String> = Vec::new();
    // Alice and Bob advertise themselves a few seconds in, then every 2 minutes; the device
    // too, so they can write to it. Alice greets at 10 s and echoes; Bob posts on Public.
    let mut next_advert = [(1usize, 3_000u64), (2, 7_000), (3, 5_000), (ME, 4_000)];
    let mut alice_hello = Some(10_000u64);
    let mut bob_post = 30_000u64;
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
        if alice_hello.is_some_and(|t| wall >= t) {
            alice_hello = None;
            say(&mut w, 1, "Hi! I'm Alice, an echo bot. Write me anything.");
        }
        if wall >= bob_post {
            bob_post += 180_000;
            say_channel(&mut w, 2, "Anyone up on the hill today?");
        }
        let echoes: Vec<String> = std::mem::take(&mut w.nodes[1].events)
            .into_iter()
            .filter_map(|e| match e {
                Event::ContactMsg { key, text, .. } if key == me_key => {
                    Some(format!("echo: {}", String::from_utf8_lossy(&text)))
                }
                _ => None,
            })
            .collect();
        for t in echoes {
            say(&mut w, 1, &t);
        }
        // Node event copies (simulator tap) are not needed beyond the bot: keep memory flat.
        w.nodes[ME].events.clear();
        w.nodes[2].events.clear();
        w.nodes[3].events.clear();
        let events = std::mem::take(&mut w.nodes[ME].ui);
        {
            let c = ctx(&w, light, ble);
            for e in &events {
                app.event(e, &c);
            }
            app.tick(&c);
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
                KeyCode::Char('p') => {
                    ble.state = match ble.state {
                        BleState::Off | BleState::Advertising => BleState::Pairing,
                        BleState::Pairing => {
                            ble.paired = ble.paired.max(1);
                            BleState::Connected
                        }
                        BleState::Connected => BleState::Advertising,
                    };
                    None
                }
                KeyCode::Char('g') => {
                    *grid = if *grid == LARGE { COMPACT } else { LARGE };
                    w.nodes[ME].dev.node.prefs.ui.compact = *grid == COMPACT;
                    app.set_grid(*grid);
                    term.clear()?;
                    None
                }
                _ => None,
            };
            gestures.extend(g);
        }
        for g in gestures {
            let c = ctx(&w, light, ble);
            app.gesture(g, &c);
            log.push(format!("{g:?}"));
        }
        for i in app.take_intents() {
            let now = w.now;
            let sn = &mut w.nodes[ME];
            let o = perform(&i, &mut sn.dev, now, &mut rng, &mut sn.st);
            log.push(format!("  → {i:?}: {o:?}"));
            if let Intent::SetPin(p) = i
                && o == Outcome::Ok
            {
                ble.pin = if p == 0 {
                    100_000 + rand_core::Rng::next_u32(&mut rng) % 900_000
                } else {
                    p
                };
            }
            match o {
                Outcome::Light => light = !light,
                Outcome::ForgetPaired => {
                    ble.paired = 0;
                    ble.state = BleState::Advertising;
                }
                Outcome::Reboot | Outcome::FactoryReset | Outcome::Hibernate => {
                    app.toast("Ignored by the emulator", ms(w.now));
                }
                _ => {}
            }
            let c = ctx(&w, light, ble);
            app.done(&i, &o, &c);
            if app.sync(&c) {
                *grid = app.grid();
                term.clear()?;
            }
        }
        if log.len() > 12 {
            log.drain(..log.len() - 12);
        }
        let c = ctx(&w, light, ble);
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
