//! The interface end to end on a simulated device: navigation, popups, overlays on the panel,
//! and a text snapshot of every screen at both grid sizes.

use mc_core::Duration;
use mc_ui::Intent;
use mc_ui::app::{App, CONFIRM_MS, Ctx};
use mc_ui::input::{Button, Gesture};
use mc_ui::panel::{self, COMPACT, Frame, Grid, LARGE};
use mc_ui::{overlay, screens};
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;

use Button::{A, B};
use Gesture::*;

/// "Tanguy" with two contacts heard (Alice, Bob), 2026-11-05 around 18:3x in Paris; Alice
/// wrote on the Public channel, then Bob wrote a DM.
fn world() -> mc_sim::World {
    let mut w = mc_sim::World::new(5);
    let me = w.add("Tanguy", |_| {});
    for name in ["Alice", "Bob"] {
        let i = w.add(name, |_| {});
        w.link(me, i);
        let now = w.now;
        w.node(i).send_self_advert(now, false).unwrap();
        w.run(Duration::from_secs(3));
    }
    // Tanguy's advert, so Bob can write to him.
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
    assert_eq!(
        w.nodes[me].dev.history().unread(),
        2,
        "Bob's DM and Alice's channel message"
    );
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
    }
}

fn text(app: &mut App, grid: Grid, c: &Ctx<'_>) -> String {
    let mut t = Terminal::new(TestBackend::new(grid.cols, grid.rows)).unwrap();
    t.draw(|f| app.draw(f, c)).unwrap();
    let buf = t.backend().buffer();
    (0..grid.rows)
        .map(|y| {
            let line: String = (0..grid.cols).map(|x| buf[(x, y)].symbol()).collect();
            format!("|{line}|")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn press(app: &mut App, c: &Ctx<'_>, gs: &[Gesture]) -> Vec<Intent> {
    for g in gs {
        app.gesture(*g, c);
    }
    app.take_intents()
}

#[test]
fn back_pops_and_double_back_goes_home() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Home → Settings (5th) → System (last) → About (first).
    press(
        &mut app,
        &c,
        &[Double(A), Long(A), Double(A), Long(A), Long(A)],
    );
    assert_eq!(app.depth(), 4);
    press(&mut app, &c, &[Click(B)]);
    assert_eq!(app.depth(), 3);
    press(&mut app, &c, &[Double(B)]);
    assert_eq!(app.depth(), 1);
    press(&mut app, &c, &[Click(B)]);
    assert_eq!(app.depth(), 1, "B on Home does nothing");
    assert_eq!(press(&mut app, &c, &[Long(B)]), vec![Intent::ToggleLight]);
}

#[test]
fn hold_to_confirm() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Settings → System → Reboot.
    press(
        &mut app,
        &c,
        &[
            Double(A),
            Long(A),
            Double(A),
            Long(A),
            Click(A),
            Click(A),
            Long(A),
        ],
    );
    assert!(app.popup().is_some());
    // Released early: nothing, and the progress starts again.
    assert!(press(&mut app, &c, &[Held(A, 1000), Held(A, 1500), HeldEnd(A)]).is_empty());
    assert!(app.popup().is_some());
    // A click does not confirm either.
    assert!(press(&mut app, &c, &[Click(A), Long(A)]).is_empty());
    let got = press(&mut app, &c, &[Held(A, 1000), Held(A, CONFIRM_MS)]);
    assert_eq!(got, vec![Intent::Reboot]);
    assert!(app.popup().is_none());
    // B cancels another one.
    press(&mut app, &c, &[Click(A), Long(A)]);
    assert!(app.popup().is_some());
    press(&mut app, &c, &[Click(B)]);
    assert!(app.popup().is_none());
    assert_eq!(app.depth(), 3, "cancelling the popup stays on System");
}

#[test]
fn advert_from_identity_shows_a_toast() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Settings → Identity (2nd) → Advert, zero hop (3rd).
    let got = press(
        &mut app,
        &c,
        &[
            Double(A),
            Long(A),
            Click(A),
            Long(A),
            Click(A),
            Click(A),
            Long(A),
        ],
    );
    assert_eq!(got, vec![Intent::SendAdvert { flood: false }]);
    let screen = text(&mut app, LARGE, &c);
    assert!(screen.contains("|Advert sent"), "{screen}");
    app.tick(c.now_ms + 3_000);
    assert!(!text(&mut app, LARGE, &c).contains("Advert sent"));
}

#[test]
fn lists_wrap_and_hold_scrolls_faster() {
    let w = world();
    let c = ctx(&w);
    let mut app = App::new(LARGE, false);
    // Home launcher: previous from the first row is the last row (Settings).
    press(&mut app, &c, &[Double(A), Long(A)]);
    assert!(text(&mut app, LARGE, &c).starts_with("|Settings"));
    press(&mut app, &c, &[Double(B)]);
    // Held 2 s and more: two rows per tick.
    press(
        &mut app,
        &c,
        &[Held(A, 1000), Held(A, 2000), HeldEnd(A), Long(A)],
    );
    // Home kept its cursor on Settings (4): 4 + 1 + 2 wraps to Channels (2).
    assert!(
        text(&mut app, LARGE, &c).starts_with("|Channels"),
        "1 + 2 rows down"
    );
}

/// On the panel, leaving Home must not leave clock pixels behind: the frame after Home → Tools
/// is exactly a fresh Tools frame.
#[test]
fn overlays_never_go_stale_on_the_panel() {
    let w = world();
    let c = ctx(&w);
    for grid in [LARGE, COMPACT] {
        let mut app = App::new(grid, true);
        let mut frame = Frame::new();
        {
            let mut t = Terminal::new(panel::backend(&mut frame, grid)).unwrap();
            t.draw(|f| app.draw(f, &c)).unwrap();
            assert_eq!(app.overlays().len(), 1, "the clock");
            for o in app.overlays() {
                overlay::draw(o, t.backend_mut().display_mut()).unwrap();
            }
            press(&mut app, &c, &[Click(A), Click(A), Click(A), Long(A)]);
            t.draw(|f| app.draw(f, &c)).unwrap();
            assert!(app.overlays().is_empty());
        }
        let mut fresh = Frame::new();
        {
            let mut app2 = App::new(grid, true);
            press(&mut app2, &c, &[Click(A), Click(A), Click(A), Long(A)]);
            let mut t = Terminal::new(panel::backend(&mut fresh, grid)).unwrap();
            t.draw(|f| app2.draw(f, &c)).unwrap();
        }
        assert!(frame == fresh, "{grid:?}: stale pixels after leaving Home");
    }
}

#[test]
fn snapshots() {
    let w = world();
    let c = ctx(&w);
    for grid in [LARGE, COMPACT] {
        for (name, gestures) in mc_ui::tour::TOUR {
            let mut app = App::new(grid, false);
            press(&mut app, &c, gestures);
            let size = if grid == LARGE { "large" } else { "compact" };
            insta::assert_snapshot!(format!("{name}_{size}"), text(&mut app, grid, &c));
        }
    }
}

#[test]
fn contact_link_matches_the_app_format() {
    let k = [0xABu8; 32];
    assert_eq!(
        screens::info_link("Zoé B", &k),
        format!(
            "meshcore://contact/add?name=Zo%C3%A9%20B&public_key={}&type=1",
            "ab".repeat(32)
        )
    );
}
