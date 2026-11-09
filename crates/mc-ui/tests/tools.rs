//! U4 tools on a simulated network: a relay that forwards, Bob who answers pings, Alice and
//! the relay with positions. Each tool works and has a snapshot.

use mc_core::Duration;
use mc_sim::World;
use mc_ui::app::{App, Ctx, GpsFix};
use mc_ui::input::{Button::*, Gesture};
use mc_ui::panel::{COMPACT, Grid, LARGE};
use mc_ui::perform;
use mc_ui::screens::tools;
use rand_core::SeedableRng;
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;

use Gesture::*;

const ME: usize = 0;

struct Rig {
    w: World,
    app: App,
    rng: rand_chacha::ChaCha20Rng,
    grid: Grid,
}

fn ctx(w: &World) -> Ctx<'_> {
    Ctx {
        dev: &w.nodes[ME].dev,
        now_ms: w
            .now
            .saturating_duration_since(mc_core::Instant::ZERO)
            .as_millis(),
        unix: w.nodes[ME].dev.node.rtc.now(w.now),
        tz_offset_min: 60,
        battery_percent: Some(80),
        charging: false,
        light: false,
        ble: mc_ui::Ble::default(),
        gps: Some(GpsFix {
            lat: 45_923_700,
            lon: 6_869_400,
            satellites: 9,
        }),
    }
}

impl Rig {
    fn new(grid: Grid) -> Self {
        let mut w = World::new(91);
        let me = w.add("Morgan", |_| {});
        let bob = w.add("Bob", |p| p.telemetry_base = 2);
        let alice = w.add("Alice", |p| {
            p.advert_loc_policy = 1;
            p.lat = 45_930_000;
            p.lon = 6_900_000;
        });
        let relay = w.add("Relay Aiguille", |p| {
            p.repeat = true;
            p.advert_loc_policy = 1;
            p.lat = 45_878_600;
            p.lon = 6_887_200;
        });
        for i in [bob, alice, relay] {
            w.link(me, i);
        }
        // The relay also answers like a MeshCore repeater (admin password "secret").
        w.serve_repeater(relay, "secret", "guest-pw");
        w.link(relay, bob);
        for x in [bob, alice, relay, me] {
            let now = w.now;
            w.node(x).send_self_advert(now, false).unwrap();
            w.run(Duration::from_secs(3));
        }
        // The relay advertises as a repeater in real life: our sim node is a companion with
        // repeating on, so mark its contact as one.
        let kr = *w.nodes[relay].dev.node.id.pub_key();
        let n = &mut w.nodes[me].dev.node;
        let s = n.contacts.find(&kr).unwrap();
        n.contacts.get_mut(s).unwrap().kind = 2;
        w.run(Duration::from_secs(90));
        Self {
            w,
            app: App::new(grid, false),
            rng: rand_chacha::ChaCha20Rng::seed_from_u64(4),
            grid,
        }
    }

    fn pump(&mut self) {
        let events = std::mem::take(&mut self.w.nodes[ME].ui);
        {
            let c = ctx(&self.w);
            for e in &events {
                self.app.event(e, &c);
            }
            self.app.tick(&c);
        }
        for i in self.app.take_intents() {
            let now = self.w.now;
            let sn = &mut self.w.nodes[ME];
            let o = perform(&i, &mut sn.dev, now, &mut self.rng, &mut sn.st);
            let c = ctx(&self.w);
            self.app.done(&i, &o, &c);
        }
    }

    fn run(&mut self, secs: u64) {
        for _ in 0..secs * 10 {
            self.w.run(Duration::from_millis(100));
            self.pump();
        }
    }

    fn press(&mut self, gs: &[Gesture]) {
        for g in gs {
            let c = ctx(&self.w);
            self.app.gesture(*g, &c);
            self.draw();
            self.pump();
        }
    }

    fn draw(&mut self) -> String {
        let mut t = Terminal::new(TestBackend::new(self.grid.cols, self.grid.rows)).unwrap();
        let c = ctx(&self.w);
        t.draw(|f| self.app.draw(f, &c)).unwrap();
        let buf = t.backend().buffer();
        (0..self.grid.rows)
            .map(|y| {
                let l: String = (0..self.grid.cols).map(|x| buf[(x, y)].symbol()).collect();
                format!("|{l}|")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Home → Tools → item `n` (0-based).
    fn tool(&mut self, n: usize) {
        self.press(&[Click(A), Click(A), Click(A), Long(A)]);
        for _ in 0..n {
            self.press(&[Click(A)]);
        }
        self.press(&[Long(A)]);
    }

    /// In a contact picker: the row whose text contains `name`.
    fn pick(&mut self, name: &str) {
        for _ in 0..10 {
            let s = self.draw();
            if s.lines()
                .any(|l| l.contains('▸') && l.contains(name) || l.contains('►') && l.contains(name))
            {
                self.press(&[Long(A)]);
                return;
            }
            self.press(&[Click(A)]);
        }
        panic!("{name} not in the picker:\n{}", self.draw());
    }
}

#[test]
fn trace_through_the_relay_reports_each_hop() {
    let mut r = Rig::new(LARGE);
    r.tool(2); // Trace route
    r.pick("Relay");
    assert!(r.draw().contains("Hold A to trace"), "{}", r.draw());
    r.press(&[Long(A)]);
    r.run(20);
    let s = r.draw();
    assert!(s.contains("Back here"), "{s}");
    assert!(s.contains("1 Relay"), "{s}");
    insta::assert_snapshot!("trace_large", s);
}

#[test]
fn range_test_pings_on_its_own_and_counts() {
    let mut r = Rig::new(LARGE);
    r.tool(3); // Ping & range
    r.pick("Bob");
    r.run(22);
    let s = r.draw();
    assert!(
        s.contains("Answered") && (s.contains("4/4 100%") || s.contains("5/5 100%")),
        "{s}"
    );
    // Pause: nothing more is sent.
    r.press(&[Long(A)]);
    let before = r.draw();
    r.run(12);
    assert!(r.draw().contains("paused"));
    let count = |s: &str| s.lines().find(|l| l.contains("Answered")).map(String::from);
    assert_eq!(count(&before), count(&r.draw()));
}

#[test]
fn gps_lists_contacts_by_distance() {
    let mut r = Rig::new(LARGE);
    r.tool(8); // GPS
    let s = r.draw();
    let alice = s.lines().position(|l| l.contains("Alice")).unwrap();
    let relay = s.lines().position(|l| l.contains("Relay")).unwrap();
    assert!(
        alice < relay,
        "Alice (2.5 km) before the relay (5 km):\n{s}"
    );
    assert!(s.contains("km NE") || s.contains("km E"), "{s}");
}

#[test]
fn snapshots() {
    for grid in [LARGE, COMPACT] {
        let size = if grid == LARGE { "large" } else { "compact" };
        for (n, name) in [
            (0, "heard"),
            (4, "signal"),
            (5, "packets"),
            (6, "airtime"),
            (8, "gps"),
            (9, "map"),
        ] {
            let mut r = Rig::new(grid);
            r.tool(n);
            insta::assert_snapshot!(format!("{name}_{size}"), r.draw());
        }
        let mut r = Rig::new(grid);
        r.tool(0);
        r.press(&[Long(A)]);
        insta::assert_snapshot!(format!("heard_detail_{size}"), r.draw());
    }
}

impl Rig {
    /// In a text entry: types `text` candidate by candidate, then the final action.
    fn type_text(&mut self, text: &str) {
        use mc_ui::keyboard::Cand;
        for ch in text.chars() {
            let want = Cand::Char(ch);
            let mut found = false;
            for _ in 0..120 {
                if self.app.keyboard().and_then(|k| k.selected()) == Some(&want) {
                    self.press(&[Long(A)]);
                    found = true;
                    break;
                }
                self.press(&[Click(A)]);
            }
            assert!(found, "{ch} not offered");
        }
        self.press(&[Double(A), Long(A)]);
    }
}

#[test]
fn discover_finds_the_relay() {
    let mut r = Rig::new(LARGE);
    r.tool(1); // Discover
    r.press(&[Long(A)]);
    r.run(5);
    let s = r.draw();
    assert!(s.contains("1 found") && s.contains("Relay"), "{s}");
    insta::assert_snapshot!("discover_large", s);
}

#[test]
fn repeater_admin_login_status_and_neighbours() {
    let mut r = Rig::new(LARGE);
    r.tool(7); // Repeater admin
    r.pick("Relay");
    // A wrong password: no answer, then failure.
    r.press(&[Click(A), Long(A)]);
    r.draw();
    r.type_text("nope");
    r.run(20);
    assert!(r.draw().contains("Login failed"), "{}", r.draw());
    // The admin password.
    r.press(&[Long(A)]);
    r.draw();
    r.type_text("secret");
    r.run(5);
    assert!(r.draw().contains("Logged in as admin"), "{}", r.draw());
    // Status.
    r.press(&[Click(A), Long(A)]);
    r.run(5);
    let s = r.draw();
    assert!(s.contains("4.05 V") && s.contains("Up"), "{s}");
    insta::assert_snapshot!("repeater_status_large", s);
    // Neighbours: the relay heard Bob, Alice and us.
    r.press(&[Click(B), Click(A), Long(A)]);
    r.run(5);
    let s = r.draw();
    assert!(
        s.contains("heard by the repeater") && s.contains("Bob"),
        "{s}"
    );
    insta::assert_snapshot!("neighbours_large", s);
}

#[test]
fn bars_follow_the_snr() {
    assert_eq!(tools::bars(48), "▂▄▆█");
    assert_eq!(tools::bars(0), "▂▄  ");
    assert_eq!(tools::bars(-40), "    ");
}
