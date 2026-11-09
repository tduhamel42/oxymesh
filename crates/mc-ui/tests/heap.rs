//! The interface's heap, screen by screen, as on the device: one long-lived terminal drawing
//! into the panel frame through mousefood, overlays included. Counted on the host, whose 64-bit
//! pointers make it an upper bound for the 32-bit ESP32-S3.
//! `cargo test -p mc-ui --test heap -- --nocapture` prints the table.
//!
//! `cap` keeps a running maximum that never resets: a screen that raises it is the worst so
//! far, and the maximum less what was held before the tour is the interface's peak.

use std::alloc::System;

use cap::Cap;
use mc_core::Duration;
use mc_ui::app::{App, Ctx};
use mc_ui::overlay;
use mc_ui::panel::{self, COMPACT, Frame, LARGE};
use ratatui_core::terminal::Terminal;

#[global_allocator]
static ALLOC: Cap<System> = Cap::new(System, usize::MAX);

/// The world of the screen tour: two contacts, an unread DM and a channel message.
fn world() -> mc_sim::World {
    let mut w = mc_sim::World::new(5);
    let me = w.add("Morgan", |_| {});
    for name in ["Alice", "Bob"] {
        let i = w.add(name, |_| {});
        w.link(me, i);
        let now = w.now;
        w.node(i).send_self_advert(now, false).unwrap();
        w.run(Duration::from_secs(3));
    }
    let now = w.now;
    w.node(me).send_self_advert(now, false).unwrap();
    w.run(Duration::from_secs(3));
    let now = w.now;
    let ts = w.node(1).time(now);
    w.node(1)
        .send_channel_text(now, 0, ts, "Snow on the pass, take chains".as_bytes())
        .unwrap();
    w.run(Duration::from_secs(5));
    let now = w.now;
    let ts = w.node(2).time(now);
    let to: [u8; 6] = w.node(me).id.pub_key()[..6].try_into().unwrap();
    w.node(2)
        .send_text(
            now,
            &to,
            0,
            0,
            ts,
            "Coffee at the refuge around 4?".as_bytes(),
        )
        .unwrap();
    w.run(Duration::from_secs(5));
    w
}

fn ctx(w: &mc_sim::World) -> Ctx<'_> {
    Ctx {
        dev: &w.nodes[0].dev,
        now_ms: 3_725_000,
        unix: mc_sim::EPOCH + 30,
        tz_offset_min: 60,
        battery_percent: Some(87),
        charging: false,
        light: false,
        ble: mc_ui::Ble::default(),
        gps: None,
    }
}

/// In host bytes, about 1.45 times the device's (the large terminal holds 30 KB here, 21 KB on
/// the M5). The M5 has 98 KB of heap, Bluetooth takes ≈ 31 KB idle and a few more connected:
/// ≈ 60 KB left for the interface, ≈ 87 KB here (`firmware/esp32s3/src/bin/mc-m5.rs`).
const BUDGET: usize = 80 * 1024;

#[test]
fn every_screen_fits_the_heap() {
    let w = world();
    let c = ctx(&w);
    let base = ALLOC.allocated();
    let before = ALLOC.max_allocated();
    let mut worst: Option<(usize, String)> = None;
    for grid in [LARGE, COMPACT] {
        let size = if grid == LARGE { "large" } else { "compact" };
        let mut frame = Frame::new();
        let mut term = Terminal::new(panel::backend(&mut frame, grid)).unwrap();
        for (name, gestures) in mc_ui::tour::TOUR {
            // From Home each time, like a person walking the menus; the terminal stays.
            let mut app = App::new(grid, true);
            app.start(&c);
            for g in *gestures {
                app.gesture(*g, &c);
                let _ = app.take_intents();
            }
            let max = ALLOC.max_allocated();
            term.draw(|f| app.draw(f, &c)).unwrap();
            for o in app.overlays() {
                let _ = overlay::draw(o, term.backend_mut().display_mut());
            }
            let held = ALLOC.allocated() - base;
            if ALLOC.max_allocated() > max {
                worst = Some((ALLOC.max_allocated() - base, format!("{name} ({size})")));
            }
            println!("{size:>7} {name:<28} held {held:>6} B");
        }
    }
    let (peak, screen) = worst.expect("the tour raised the maximum");
    assert!(
        ALLOC.max_allocated() > before,
        "building the world peaked higher than the tour: no measure"
    );
    println!("peak {peak} B, on {screen}");
    assert!(peak <= BUDGET, "{peak} B on {screen}");
}
