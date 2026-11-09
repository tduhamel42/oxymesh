//! U5: every setting goes through the device's settings API, round-trips through storage, and
//! the app sees it (SELF_INFO and the other getters). Plus the screens themselves.

use mc_core::Duration;
use mc_proto::{Command, Response};
use mc_sim::World;
use mc_ui::app::{App, CONFIRM_MS, Ctx};
use mc_ui::input::{Button::*, Gesture, Gesture::*};
use mc_ui::panel::{COMPACT, Grid, LARGE};
use mc_ui::{Intent, Outcome, perform};
use rand_core::SeedableRng;
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;

const ME: usize = 0;

fn ctx(w: &World) -> Ctx<'_> {
    let dev = &w.nodes[ME].dev;
    Ctx {
        dev,
        now_ms: w
            .now
            .saturating_duration_since(mc_core::Instant::ZERO)
            .as_millis(),
        unix: dev.node.rtc.now(w.now),
        tz_offset_min: dev.node.prefs.ui.tz_offset_min,
        battery_percent: Some(80),
        charging: false,
        light: false,
        ble: mc_ui::Ble::default(),
        gps: None,
    }
}

fn world() -> World {
    let mut w = World::new(51);
    w.add("Morgan", |_| {});
    w
}

fn enc(c: &Command<'_>) -> Vec<u8> {
    let mut b = [0u8; 256];
    let n = c.encode(&mut b).unwrap();
    b[..n].to_vec()
}

fn ask(w: &mut World, c: &Command<'_>) -> Vec<u8> {
    w.app(ME, &enc(c)).remove(0).to_vec()
}

fn apply(w: &mut World, i: Intent) {
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(1);
    let now = w.now;
    let sn = &mut w.nodes[ME];
    assert_eq!(
        perform(&i, &mut sn.dev, now, &mut rng, &mut sn.st),
        Outcome::Ok,
        "{i:?}"
    );
}

/// What storage holds decodes to what the device uses.
fn stored_matches(w: &World) {
    let sn = &w.nodes[ME];
    let rec = sn.st.prefs.as_ref().expect("prefs saved");
    let back = mc_node::persist::decode_prefs(rec, &sn.dev.node.prefs).unwrap();
    assert_eq!(
        mc_node::persist::encode_prefs(&back),
        mc_node::persist::encode_prefs(&sn.dev.node.prefs)
    );
}

#[test]
fn every_setting_round_trips_and_the_app_sees_it() {
    let mut w = world();
    ask(&mut w, &Command::DeviceQuery { app_ver: 3 });
    let self_info = |w: &mut World| {
        ask(
            w,
            &Command::AppStart {
                reserved: [0; 7],
                app_name: b"t",
            },
        )
    };

    apply(&mut w, Intent::SetName("Morgan M5".into()));
    apply(
        &mut w,
        Intent::SetRadio {
            freq_khz: 869_525,
            bw_hz: 250_000,
            sf: 11,
            cr: 5,
        },
    );
    apply(&mut w, Intent::SetTxPower(14));
    apply(
        &mut w,
        Intent::SetLocation {
            lat: 45_923_700,
            lon: 6_869_400,
        },
    );
    apply(
        &mut w,
        Intent::SetOther {
            manual_add: 1,
            telemetry: 2 | (1 << 2),
            advert_loc: 1,
            multi_acks: 1,
        },
    );
    stored_matches(&w);
    match Response::decode(&self_info(&mut w)).unwrap() {
        Response::SelfInfo(si) => {
            assert_eq!(si.node_name, b"Morgan M5");
            assert_eq!(
                (si.freq_khz, si.bw_hz, si.sf, si.cr),
                (869_525, 250_000, 11, 5)
            );
            assert_eq!(si.tx_power, 14);
            assert_eq!((si.lat, si.lon), (45_923_700, 6_869_400));
            assert_eq!(
                (
                    si.manual_add,
                    si.telemetry,
                    si.advert_loc_policy,
                    si.multi_acks
                ),
                (1, 6, 1, 1)
            );
        }
        other => panic!("{other:?}"),
    }

    apply(
        &mut w,
        Intent::SetAutoadd {
            config: 0x06,
            max_hops: 3,
        },
    );
    assert!(matches!(
        Response::decode(&ask(&mut w, &Command::GetAutoaddConfig)).unwrap(),
        Response::AutoaddConfig {
            config: 0x06,
            max_hops: 3
        }
    ));
    apply(&mut w, Intent::SetPathHash(1));
    apply(&mut w, Intent::SetPin(123_456));
    match Response::decode(&ask(&mut w, &Command::DeviceQuery { app_ver: 3 })).unwrap() {
        Response::DeviceInfo(d) => assert_eq!((d.path_hash_mode, d.ble_pin), (1, 123_456)),
        other => panic!("{other:?}"),
    }
    apply(&mut w, Intent::SetScope(Some("fr".into())));
    match Response::decode(&ask(&mut w, &Command::GetDefaultFloodScope)).unwrap() {
        Response::DefaultFloodScope {
            scope: Some((name, key)),
        } => {
            assert_eq!(&name[..2], b"fr");
            assert_eq!(key, mc_air::crypto::region_key("fr"));
        }
        other => panic!("{other:?}"),
    }
    // The simulated device has no GPS: refused like the C++ (`gps` custom variable).
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(1);
    let now = w.now;
    let sn = &mut w.nodes[ME];
    assert_eq!(
        perform(
            &Intent::SetGps(true),
            &mut sn.dev,
            now,
            &mut rng,
            &mut sn.st
        ),
        Outcome::Failed(mc_proto::codec::err::ILLEGAL_ARG)
    );
    w.nodes[ME].dev.info.has_gps = true;
    apply(&mut w, Intent::SetGps(true));
    match Response::decode(&ask(&mut w, &Command::GetCustomVars)).unwrap() {
        Response::CustomVars { vars } => {
            assert!(
                String::from_utf8_lossy(vars).contains("gps:1"),
                "{:?}",
                String::from_utf8_lossy(vars)
            )
        }
        other => panic!("{other:?}"),
    }
    apply(&mut w, Intent::SetGpsInterval(300));
    assert_eq!(w.nodes[ME].dev.node.prefs.gps_interval, 300);
    // The screen's own settings: storage only, the app never sees them.
    let mut u = w.nodes[ME].dev.node.prefs.ui;
    u.compact = true;
    u.light_timeout_s = 600;
    u.buzzer = 1;
    u.led_unread = false;
    u.tz_offset_min = -210;
    u.invert = true;
    u.rotate = true;
    apply(&mut w, Intent::SetUi(u));
    stored_matches(&w);
    let rec = w.nodes[ME].st.prefs.clone().unwrap();
    let back = mc_node::persist::decode_prefs(&rec, &mc_node::Prefs::ZERO).unwrap();
    assert_eq!(back.ui, u);
}

// ------------------------------------------------------------------ the screens

struct Rig {
    w: World,
    app: App,
    rng: rand_chacha::ChaCha20Rng,
}

impl Rig {
    fn new(grid: Grid) -> Self {
        let mut w = world();
        w.nodes[ME].dev.node.prefs.ui.compact = grid == COMPACT;
        Self {
            w,
            app: App::new(grid, false),
            rng: rand_chacha::ChaCha20Rng::seed_from_u64(2),
        }
    }

    fn press(&mut self, gs: &[Gesture]) {
        for g in gs {
            let c = ctx(&self.w);
            self.app.gesture(*g, &c);
            self.draw();
            for i in self.app.take_intents() {
                let now = self.w.now;
                let sn = &mut self.w.nodes[ME];
                let o = perform(&i, &mut sn.dev, now, &mut self.rng, &mut sn.st);
                let c = ctx(&self.w);
                self.app.done(&i, &o, &c);
                self.app.sync(&c);
            }
        }
        self.w.run(Duration::from_millis(100));
    }

    fn draw(&mut self) -> String {
        let g = self.app.grid();
        let mut t = Terminal::new(TestBackend::new(g.cols, g.rows)).unwrap();
        let c = ctx(&self.w);
        t.draw(|f| self.app.draw(f, &c)).unwrap();
        let buf = t.backend().buffer();
        (0..g.rows)
            .map(|y| {
                format!(
                    "|{}|",
                    (0..g.cols)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Home → Settings → item `n`.
    fn settings(&mut self, n: usize) {
        self.press(&[Double(A), Long(A)]);
        for _ in 0..n {
            self.press(&[Click(A)]);
        }
        self.press(&[Long(A)]);
    }
}

#[test]
fn spreading_factor_needs_hold_to_confirm() {
    let mut r = Rig::new(LARGE);
    r.settings(0); // Radio
    r.press(&[Click(A), Click(A), Click(A), Long(A)]); // Spreading
    assert!(r.draw().contains("SF8"));
    r.press(&[Click(A), Click(A), Long(A)]); // SF10
    assert!(r.app.popup().is_some(), "{}", r.draw());
    assert_eq!(
        r.w.nodes[ME].dev.node.prefs.radio.sf, 8,
        "not before the hold"
    );
    r.press(&[Held(A, 1000), Held(A, CONFIRM_MS)]);
    assert_eq!(r.w.nodes[ME].dev.node.prefs.radio.sf, 10);
    assert_eq!(
        r.w.nodes[ME].dev.poll_action(),
        Some(mc_node::Action::RadioChanged)
    );
    assert!(
        r.draw().contains("Radio") && r.draw().contains("SF10"),
        "back on the radio menu:\n{}",
        r.draw()
    );
}

#[test]
fn text_size_switches_the_grid() {
    let mut r = Rig::new(LARGE);
    r.settings(4); // Display
    r.press(&[Long(A), Click(A), Long(A)]); // Text size → Compact
    assert_eq!(r.app.grid(), COMPACT);
    assert!(r.w.nodes[ME].dev.node.prefs.ui.compact);
}

#[test]
fn toggles_flip_and_save() {
    let mut r = Rig::new(LARGE);
    r.settings(1); // Identity
    r.press(&[Click(A), Click(A), Long(A)]); // Share location
    assert_eq!(r.w.nodes[ME].dev.node.prefs.advert_loc_policy, 1);
    r.press(&[Long(A)]);
    assert_eq!(r.w.nodes[ME].dev.node.prefs.advert_loc_policy, 0);
}

#[test]
fn fixed_pin_is_typed_with_digits() {
    let mut r = Rig::new(LARGE);
    r.settings(2); // Bluetooth
    r.press(&[Click(A), Long(A)]); // Status, then PIN
    r.draw();
    assert!(r.app.keyboard().is_some(), "{}", r.draw());
    for d in "246810".chars() {
        let want = mc_ui::keyboard::Cand::Char(d);
        while r.app.keyboard().and_then(|k| k.selected()) != Some(&want) {
            r.press(&[Click(A)]);
        }
        r.press(&[Long(A)]);
    }
    // Six digits: Save is first.
    r.press(&[Long(A)]);
    assert_eq!(r.w.nodes[ME].dev.node.prefs.ble_pin, 246_810);
}

#[test]
fn snapshots() {
    for grid in [LARGE, COMPACT] {
        let size = if grid == LARGE { "large" } else { "compact" };
        for (n, name) in [
            (0, "radio"),
            (1, "identity"),
            (2, "bluetooth"),
            (3, "contacts"),
            (4, "display"),
            (5, "notifications"),
            (6, "telemetry"),
            (7, "power"),
            (8, "time"),
        ] {
            let mut r = Rig::new(grid);
            r.settings(n);
            insta::assert_snapshot!(format!("{name}_{size}"), r.draw());
        }
        let mut r = Rig::new(grid);
        r.settings(8);
        r.press(&[Long(A)]); // Time zone: long list, current first
        insta::assert_snapshot!(format!("timezone_{size}"), r.draw());
    }
}
