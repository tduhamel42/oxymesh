//! The status pages of the one-button OLED (Heltec V4, `docs/spec/ui.md` §12).

use mc_core::Duration;
use mc_ui::app::Ctx;
use mc_ui::input::{Button, Gesture};
use mc_ui::status::{self, Oled, Page, Status};
use mc_ui::{Ble, BleState, Intent, Outcome};

use Gesture::*;

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
    // Morgan's advert, so Bob can write to them.
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
    w.nodes[me].dev.noise_floor = -112;
    w
}

fn ctx(w: &mc_sim::World, ble: Ble) -> Ctx<'_> {
    Ctx {
        dev: &w.nodes[0].dev,
        now_ms: 3_725_000,
        unix: mc_sim::EPOCH + 30,
        tz_offset_min: 60,
        battery_percent: Some(87),
        charging: false,
        light: true,
        ble,
        gps: None,
    }
}

fn text(s: &Status, c: &Ctx<'_>) -> String {
    let sc = s.screen(c);
    let mut lines: Vec<String> = sc.lines.iter().map(|l| format!("|{l:<21}|")).collect();
    if let Some(b) = sc.big {
        lines.push(format!("BIG {b}"));
    }
    lines.join("\n")
}

/// The OLED as half blocks, two pixel rows per text line.
fn pixels(f: &Oled) -> String {
    (0..status::HEIGHT / 2)
        .map(|r| {
            (0..status::WIDTH)
                .map(|x| match (f.lit(x, 2 * r), f.lit(x, 2 * r + 1)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_page() {
    let w = world();
    let c = ctx(
        &w,
        Ble {
            state: BleState::Connected,
            pin: 0,
            paired: 1,
        },
    );
    let mut s = Status::new();
    let mut all = String::new();
    for _ in 0..6 {
        all.push_str(&text(&s, &c));
        all.push_str("\n\n");
        assert_eq!(s.gesture(Click(Button::A), &c), None);
    }
    assert_eq!(s.page(), Page::Home, "the pages go round");
    insta::assert_snapshot!(all);
}

#[test]
fn gestures_and_actions() {
    let w = world();
    let c = ctx(&w, Ble::default());
    let mut s = Status::new();
    s.gesture(Double(Button::A), &c);
    assert_eq!(s.page(), Page::Off);
    assert_eq!(s.gesture(Long(Button::A), &c), Some(Intent::Hibernate));
    s.gesture(Double(Button::A), &c);
    assert_eq!(s.page(), Page::Advert);
    let i = s.gesture(HeldEnd(Button::A), &c).unwrap();
    assert_eq!(i, Intent::SendAdvert { flood: true });
    s.done(&i, &Outcome::Ok, 1000);
    assert!(text(&s, &c).contains("> Advert sent"));
    assert!(!s.tick(2000));
    assert!(s.tick(3500), "the note goes away by itself");
    assert!(!text(&s, &c).contains("Advert sent"));
    // Messages: a long press marks the newest unread conversation read.
    for _ in 0..3 {
        s.gesture(Click(Button::A), &c);
    }
    assert_eq!(s.page(), Page::Messages);
    assert!(matches!(
        s.gesture(Long(Button::A), &c),
        Some(Intent::MarkRead(_))
    ));
    // Button B does nothing here: the board has one button.
    assert_eq!(s.gesture(Click(Button::B), &c), None);
}

#[test]
fn pairing_pin_covers_the_page() {
    let w = world();
    let c = ctx(
        &w,
        Ble {
            state: BleState::Pairing,
            pin: 42_917,
            paired: 0,
        },
    );
    let s = Status::new();
    assert!(text(&s, &c).ends_with("BIG 042917"));
    let mut f = Oled::new();
    s.draw(&mut f, &c);
    insta::assert_snapshot!(pixels(&f));
}

#[test]
fn home_on_the_oled() {
    let w = world();
    let c = ctx(
        &w,
        Ble {
            state: BleState::Advertising,
            pin: 0,
            paired: 0,
        },
    );
    let mut f = Oled::new();
    Status::new().draw(&mut f, &c);
    insta::assert_snapshot!(pixels(&f));
}
