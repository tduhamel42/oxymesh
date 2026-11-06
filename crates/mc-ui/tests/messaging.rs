//! U3 end to end: the real App on a simulated device, with an echo bot. Read the bot's
//! message, reply by keyboard and by quick reply, see both delivered.

use mc_core::Duration;
use mc_node::Event;
use mc_node::history::{Peer, Status};
use mc_sim::World;
use mc_ui::app::{App, Ctx};
use mc_ui::input::{Button::*, Gesture::*};
use mc_ui::keyboard::Cand;
use mc_ui::panel::LARGE;
use mc_ui::perform;
use rand_core::SeedableRng;
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;

const ME: usize = 0;
const ALICE: usize = 1;

struct Rig {
    w: World,
    app: App,
    rng: rand_chacha::ChaCha20Rng,
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
        gps: None,
    }
}

impl Rig {
    fn new() -> Self {
        let mut w = World::new(77);
        let me = w.add("Tanguy", |_| {});
        let alice = w.add("Alice", |_| {});
        w.link(me, alice);
        for (x, y) in [(me, alice), (alice, me)] {
            let now = w.now;
            w.node(x).send_self_advert(now, false).unwrap();
            let kx = *w.node(x).id.pub_key();
            assert!(w.run_until(Duration::from_secs(10), |w| {
                w.nodes[y].dev.node.contacts.find(&kx).is_some()
            }));
        }
        Self {
            w,
            app: App::new(LARGE, false),
            rng: rand_chacha::ChaCha20Rng::seed_from_u64(3),
        }
    }

    /// The runner: notifications to the App, its intents to the device, and the echo bot.
    fn pump(&mut self) {
        let events = std::mem::take(&mut self.w.nodes[ME].ui);
        {
            let c = ctx(&self.w);
            for e in &events {
                self.app.event(e, &c);
            }
        }
        for i in self.app.take_intents() {
            let now = self.w.now;
            let sn = &mut self.w.nodes[ME];
            let o = perform(&i, &mut sn.dev, now, &mut self.rng, &mut sn.st);
            let c = ctx(&self.w);
            self.app.done(&i, &o, &c);
        }
        // Echo bot: Alice answers every message from Tanguy.
        let km: [u8; 32] = *self.w.nodes[ME].dev.node.id.pub_key();
        let got: Vec<Vec<u8>> = std::mem::take(&mut self.w.nodes[ALICE].events)
            .into_iter()
            .filter_map(|e| match e {
                Event::ContactMsg { key, text, .. } if key == km => Some(text.to_vec()),
                _ => None,
            })
            .collect();
        for t in got {
            self.alice_says(&format!("echo: {}", String::from_utf8_lossy(&t)));
        }
    }

    fn alice_says(&mut self, text: &str) {
        let now = self.w.now;
        let ts = self.w.node(ALICE).time(now);
        let to: [u8; 6] = self.w.node(ME).id.pub_key()[..6].try_into().unwrap();
        self.w
            .node(ALICE)
            .send_text(now, &to, 0, 0, ts, text.as_bytes())
            .unwrap();
    }

    fn run(&mut self, secs: u64, until: impl Fn(&World) -> bool) -> bool {
        for _ in 0..secs * 10 {
            self.w.run(Duration::from_millis(100));
            self.pump();
            if until(&self.w) {
                return true;
            }
        }
        false
    }

    fn press(&mut self, gs: &[mc_ui::input::Gesture]) {
        for g in gs {
            let c = ctx(&self.w);
            self.app.gesture(*g, &c);
            self.draw();
        }
        self.pump();
    }

    fn draw(&mut self) -> String {
        let mut t = Terminal::new(TestBackend::new(LARGE.cols, LARGE.rows)).unwrap();
        let c = ctx(&self.w);
        t.draw(|f| self.app.draw(f, &c)).unwrap();
        let buf = t.backend().buffer();
        (0..LARGE.rows)
            .map(|y| {
                (0..LARGE.cols)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Moves to the candidate `want` with A clicks and inserts it with a long A.
    fn type_cand(&mut self, want: &Cand) {
        for _ in 0..200 {
            if self.app.keyboard().and_then(|k| k.selected()) == Some(want) {
                self.press(&[Long(A)]);
                return;
            }
            self.press(&[Click(A)]);
        }
        panic!("candidate {want:?} not found");
    }

    fn last_sent(&self) -> Status {
        self.w.nodes[ME]
            .dev
            .history()
            .iter()
            .filter(|m| m.outgoing)
            .last()
            .unwrap()
            .status
    }
}

#[test]
fn read_the_bot_and_reply_by_keyboard_and_by_quick_reply() {
    let mut r = Rig::new();
    let alice = Peer::Contact(
        r.w.nodes[ALICE].dev.node.id.pub_key()[..6]
            .try_into()
            .unwrap(),
    );
    r.alice_says("Hi! I'm Alice, an echo bot. Write me anything.");
    assert!(r.run(30, |w| w.nodes[ME].dev.history().unread() == 1));
    // The banner shows it; the first press opens the conversation and reads it.
    assert!(r.app.banner().is_some());
    assert!(r.draw().contains("New message: Alice"), "{}", r.draw());
    r.press(&[Click(A)]);
    assert_eq!(r.w.nodes[ME].dev.history().unread_with(alice), 0);
    let screen = r.draw();
    assert!(screen.contains("Write me anything"), "{screen}");

    // Reply by keyboard: "Hi" then Send.
    r.press(&[Long(A)]);
    r.draw();
    r.type_cand(&Cand::Char('H'));
    r.type_cand(&Cand::Char('i'));
    assert_eq!(r.app.keyboard().unwrap().text, "Hi");
    r.press(&[Double(A)]);
    assert_eq!(
        r.app.keyboard().unwrap().selected(),
        Some(&Cand::Done),
        "Send is one double click away"
    );
    r.press(&[Long(A)]);
    assert!(r.run(60, |w| matches!(
        w.nodes[ME].dev.history().iter().filter(|m| m.outgoing).last().unwrap().status,
        Status::Delivered { .. }
    )));
    // The bot's echo arrives in the open conversation: read at once.
    assert!(r.run(60, |w| {
        w.nodes[ME]
            .dev
            .history()
            .with(alice)
            .any(|m| &m.text[..] == b"echo: Hi")
    }));
    r.pump();
    assert_eq!(r.w.nodes[ME].dev.history().unread_with(alice), 0);
    let screen = r.draw();
    assert!(screen.contains("delivered"), "{screen}");
    assert!(screen.contains("echo: Hi"), "{screen}");

    // Quick reply: hold A to write, hold A on "OK", hold A on Send.
    r.press(&[Long(A)]);
    r.draw();
    r.press(&[Long(A), Long(A)]);
    assert!(matches!(r.last_sent(), Status::Sending { .. }));
    assert!(r.run(60, |w| matches!(
        w.nodes[ME].dev.history().iter().filter(|m| m.outgoing).last().unwrap().status,
        Status::Delivered { .. }
    )));
    let sent = r.w.nodes[ME]
        .dev
        .history()
        .iter()
        .filter(|m| m.outgoing)
        .last()
        .unwrap()
        .clone();
    assert_eq!(&sent.text[..], b"OK");
}

#[test]
fn first_boot_runs_the_setup_and_it_can_be_skipped() {
    let mut r = Rig::new();
    let c = ctx(&r.w);
    r.app.start(&c);
    assert!(r.draw().contains("1/3 Region"));
    // Keep current → name (B on the unchanged name keeps it) → time → finish.
    r.press(&[Long(A)]);
    assert!(r.draw().contains("2/3 Name"));
    r.press(&[Double(A), Long(A)]);
    assert!(r.draw().contains("3/3 Time"), "{}", r.draw());
    r.press(&[Long(A)]);
    assert_eq!(r.app.depth(), 1);
    assert!(r.w.nodes[ME].dev.node.prefs.ui.setup_done);
    // Next boot: no setup.
    let mut again = App::new(LARGE, false);
    let c = ctx(&r.w);
    again.start(&c);
    assert_eq!(again.depth(), 1);
}
