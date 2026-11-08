//! Which device event plays which buzzer tune (`docs/spec/ui.md` §8), on a simulated pair.

use mc_core::Duration;
use mc_node::device::UiEvent;
use mc_node::history::{Peer, Status};
use mc_sim::World;
use mc_ui::tune::{Alert, Level, alert};

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

/// Alice (0) and Bob (1) know each other and share `#oxymesh`; returns its slot.
fn pair() -> (World, u8) {
    let mut w = World::new(9);
    let a = w.add("Alice", |_| {});
    let b = w.add("Bob", |_| {});
    w.link(a, b);
    for (x, y) in [(a, b), (b, a)] {
        let now = w.now;
        w.node(x).send_self_advert(now, false).unwrap();
        let kx = *w.node(x).id.pub_key();
        assert!(w.run_until(secs(10), |w| {
            w.nodes[y].dev.node.contacts.find(&kx).is_some()
        }));
    }
    let mut idx = 0;
    for i in [a, b] {
        let sn = &mut w.nodes[i];
        idx = sn.dev.add_hashtag_channel("#oxymesh", &mut sn.st).unwrap();
    }
    (w, idx)
}

/// The tunes Alice's events since the last call ask for.
fn tunes(w: &mut World, phone: bool) -> Vec<(Alert, Level)> {
    let events: Vec<UiEvent> = std::mem::take(&mut w.nodes[0].ui);
    events
        .iter()
        .filter_map(|e| alert(e, &w.nodes[0].dev, phone))
        .collect()
}

fn bob_dm(w: &mut World, text: &str) {
    let now = w.now;
    let ts = w.node(1).time(now);
    let ka: [u8; 6] = w.node(0).id.pub_key()[..6].try_into().unwrap();
    w.node(1)
        .send_text(now, &ka, 0, 0, ts, text.as_bytes())
        .unwrap();
    let n = w.nodes[0].dev.history().unread();
    assert!(w.run_until(secs(30), |w| w.nodes[0].dev.history().unread() > n));
}

fn bob_channel(w: &mut World, idx: u8, text: &str) {
    let now = w.now;
    let ts = w.node(1).time(now);
    w.node(1)
        .send_channel_text(now, idx, ts, text.as_bytes())
        .unwrap();
    let n = w.nodes[0].dev.history().unread();
    assert!(w.run_until(secs(30), |w| w.nodes[0].dev.history().unread() > n));
}

fn set_ui(w: &mut World, f: impl FnOnce(&mut mc_node::node::UiPrefs)) {
    let sn = &mut w.nodes[0];
    let mut u = sn.dev.node.prefs.ui;
    f(&mut u);
    sn.dev.set_ui(u, &mut sn.st);
}

#[test]
fn each_event_plays_its_tune() {
    let (mut w, idx) = pair();
    let _ = tunes(&mut w, false);

    bob_dm(&mut w, "hi");
    assert_eq!(tunes(&mut w, false), [(Alert::Dm, Level::Loud)]);
    bob_channel(&mut w, idx, "all of you");
    assert_eq!(tunes(&mut w, false), [(Alert::Channel, Level::Loud)]);

    // Sent from the screen and delivered: the ack beep, once.
    let kb = *w.nodes[1].dev.node.id.pub_key();
    let now = w.now;
    let sn = &mut w.nodes[0];
    let seq = sn.dev.send_text(now, &kb, b"got it", &mut sn.st).unwrap();
    assert!(w.run_until(secs(30), |w| matches!(
        w.nodes[0].dev.history().get(seq).unwrap().status,
        Status::Delivered { .. }
    )));
    assert_eq!(tunes(&mut w, false), [(Alert::Delivered, Level::Loud)]);

    // Reading a conversation changes statuses but plays nothing.
    let sn = &mut w.nodes[0];
    sn.dev.mark_read(Peer::Channel(idx), &mut sn.st);
    assert_eq!(tunes(&mut w, false), []);
}

#[test]
fn muted_quiet_off_and_phone() {
    let (mut w, idx) = pair();
    let _ = tunes(&mut w, false);

    set_ui(&mut w, |u| *u = u.with_muted(idx, true));
    bob_channel(&mut w, idx, "muted");
    assert_eq!(tunes(&mut w, false), [], "a muted channel stays silent");
    bob_dm(&mut w, "still calls");
    assert_eq!(tunes(&mut w, false), [(Alert::Dm, Level::Loud)]);

    set_ui(&mut w, |u| u.buzzer = 1);
    bob_dm(&mut w, "softly");
    assert_eq!(tunes(&mut w, false), [(Alert::Dm, Level::Quiet)]);

    bob_dm(&mut w, "the phone rings instead");
    assert_eq!(tunes(&mut w, true), []);

    set_ui(&mut w, |u| u.buzzer = 0);
    bob_dm(&mut w, "silence");
    assert_eq!(tunes(&mut w, false), []);
    assert!(
        w.nodes[0].dev.node.prefs.buzzer_quiet,
        "the C++ flag follows"
    );
}
