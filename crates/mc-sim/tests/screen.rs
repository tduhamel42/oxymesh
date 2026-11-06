//! What the device keeps for its own screen (U2): message history and unread state, heard
//! nodes, and the offline queue in storage, which survives a reboot.

use mc_core::Duration;
use mc_node::device::UiEvent;
use mc_node::history::{Peer, Status};
use mc_node::{Device, Event};
use mc_proto::{Command, Response};
use mc_sim::{SIM_INFO, World};

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

fn enc(c: &Command<'_>) -> Vec<u8> {
    let mut b = [0u8; 256];
    let n = c.encode(&mut b).unwrap();
    b[..n].to_vec()
}

/// Alice (0) and Bob (1) know each other; Bob sends Alice `text`.
fn bob_writes(text: &str) -> World {
    let mut w = World::new(21);
    let a = w.add("Alice", |_| {});
    let b = w.add("Bob", |_| {});
    w.link(a, b);
    for (x, y) in [(a, b), (b, a)] {
        let now = w.now;
        w.node(x).send_self_advert(now, false).unwrap();
        let kx = *w.node(x).id.pub_key();
        assert!(w.run_until(secs(10), |w| {
            w.nodes[y]
                .events
                .iter()
                .any(|e| matches!(e, Event::Advert { key, .. } if *key == kx))
        }));
    }
    let now = w.now;
    let ts = w.node(b).time(now);
    let ka: [u8; 6] = w.node(a).id.pub_key()[..6].try_into().unwrap();
    w.node(b)
        .send_text(now, &ka, 0, 0, ts, text.as_bytes())
        .unwrap();
    assert!(w.run_until(secs(30), |w| w.nodes[a].dev.history().unread() == 1));
    w
}

fn is_dm(f: &[u8]) -> bool {
    matches!(
        Response::decode(f).unwrap(),
        Response::ContactMsg { .. } | Response::ContactMsgLegacy { .. }
    )
}

fn bob(w: &World) -> Peer {
    Peer::Contact(w.nodes[1].dev.node.id.pub_key()[..6].try_into().unwrap())
}

#[test]
fn received_message_is_unread_until_the_app_fetches_it() {
    let mut w = bob_writes("hello — from Bob");
    let peer = bob(&w);
    let m = w.nodes[0].dev.history().with(peer).next().unwrap().clone();
    assert_eq!(&m.text[..], "hello — from Bob".as_bytes());
    assert_eq!(m.status, Status::Unread);
    assert!(
        w.nodes[0]
            .ui
            .iter()
            .any(|e| matches!(e, UiEvent::Message { seq, outgoing: false, .. } if *seq == m.seq))
    );
    // The heard ring names Bob from his advert.
    let h = w.nodes[0].dev.heard().last().unwrap();
    assert_eq!(&h.name[..3], b"Bob");

    // The app fetches it: read on the screen too.
    let out = w.app(0, &enc(&Command::SyncNextMessage));
    assert!(is_dm(&out[0]));
    assert_eq!(w.nodes[0].dev.history().unread(), 0);
    assert!(w.nodes[0].ui.contains(&UiEvent::Status { seq: m.seq }));
    let out = w.app(0, &enc(&Command::SyncNextMessage));
    assert!(matches!(
        Response::decode(&out[0]).unwrap(),
        Response::NoMoreMessages
    ));
}

#[test]
fn viewing_on_the_device_marks_read_and_the_app_still_gets_it() {
    let mut w = bob_writes("read me here");
    let peer = bob(&w);
    let sn = &mut w.nodes[0];
    sn.dev.mark_read(peer, &mut sn.st);
    assert_eq!(sn.dev.history().unread(), 0);
    // The protocol's queue is untouched by the screen.
    assert_eq!(sn.dev.queued_messages(), 1);
    let out = w.app(0, &enc(&Command::SyncNextMessage));
    assert!(is_dm(&out[0]));
}

#[test]
fn queue_and_history_survive_a_reboot() {
    let w = bob_writes("still here after reboot");
    let st = w.nodes[0].st.clone();
    // A new device from storage alone, the way the firmware reloads it.
    let node = mc_node::Node::new(
        w.nodes[0].dev.node.id.clone(),
        w.nodes[0].dev.node.prefs.clone(),
        mc_sim::EPOCH,
        w.now,
    );
    let mut dev = Device::new(node, SIM_INFO);
    dev.connected = true;
    for rec in st.history.values() {
        dev.restore_history(rec);
    }
    for (seq, entry) in &st.queued {
        dev.restore_queued(*seq, entry);
    }
    assert_eq!(dev.history().unread(), 1);
    assert_eq!(dev.queued_messages(), 1);
    let mut st = st;
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(1);
    dev.on_app_frame(w.now, &enc(&Command::SyncNextMessage), &mut rng, &mut st);
    let f = dev.poll_app().unwrap();
    match Response::decode(&f).unwrap() {
        // No DEVICE_QUERY yet: the legacy frame, like the C++ firmware.
        Response::ContactMsgLegacy { text, .. } => assert_eq!(text, b"still here after reboot"),
        other => panic!("{other:?}"),
    }
    assert_eq!(dev.history().unread(), 0);
    assert!(st.queued.is_empty(), "fetched entries leave the storage");
}

use rand_core::SeedableRng;

// ------------------------------------------------------------------ sending from the screen

use mc_node::device::ReqKind;

fn key(w: &World, i: usize) -> [u8; 32] {
    *w.nodes[i].dev.node.id.pub_key()
}

fn status(w: &World, i: usize, seq: u32) -> Status {
    w.nodes[i].dev.history().get(seq).unwrap().status
}

/// Frames pushed to node `i`'s app with this code.
fn pushed(w: &World, i: usize, code: u8) -> usize {
    w.nodes[i]
        .app
        .iter()
        .filter(|f| f.first() == Some(&code))
        .count()
}

#[test]
fn screen_message_is_delivered_and_the_app_does_not_hear_the_ack() {
    let mut w = bob_writes("first");
    let kb = key(&w, 1);
    let now = w.now;
    let sn = &mut w.nodes[0];
    let seq = sn
        .dev
        .send_text(now, &kb, b"from the screen", &mut sn.st)
        .unwrap();
    assert_eq!(status(&w, 0, seq), Status::Sending { attempt: 0 });
    assert!(w.run_until(secs(30), |w| matches!(
        status(w, 0, seq),
        Status::Delivered { .. }
    )));
    assert_eq!(pushed(&w, 0, 0x82), 0, "no SEND_CONFIRMED for the app");
    assert!(
        w.nodes[1].events.iter().any(
            |e| matches!(e, Event::ContactMsg { text, .. } if **text == b"from the screen"[..])
        )
    );
}

#[test]
fn unreachable_contact_gets_three_tries_then_a_flood_then_failed() {
    let mut w = bob_writes("first");
    let kb = key(&w, 1);
    w.unlink(0, 1);
    w.nodes[0].ui.clear();
    let now = w.now;
    let sn = &mut w.nodes[0];
    let seq = sn.dev.send_text(now, &kb, b"anyone?", &mut sn.st).unwrap();
    assert!(w.run_until(secs(300), |w| status(w, 0, seq) == Status::Failed));
    // Each try went out as a TXT_MSG: 3 by the known route, then 1 flood.
    let tx: Vec<_> = w
        .log
        .iter()
        .filter(|(_, from, raw)| {
            *from == 0
                && mc_air::Packet::decode(raw)
                    .is_ok_and(|p| p.header.ptype == mc_air::PayloadType::TXT_MSG)
        })
        .collect();
    assert_eq!(tx.len(), 4, "3 tries and a flood");
    let last = mc_air::Packet::decode(&tx[3].2).unwrap();
    assert!(last.header.route.is_flood(), "the last try floods");
    let c = w.nodes[0].dev.node.contacts.find(&kb).unwrap();
    assert_eq!(
        w.nodes[0].dev.node.contacts.get(c).unwrap().out_path_len,
        mc_node::contacts::PATH_UNKNOWN,
        "RESET_PATH before the flood"
    );
    // The screen was told at each step: 3 retries and the failure.
    let told = w.nodes[0]
        .ui
        .iter()
        .filter(|e| matches!(e, UiEvent::Status { seq: s } if *s == seq))
        .count();
    assert_eq!(told, 4);
}

#[test]
fn a_late_ack_of_an_earlier_try_still_counts() {
    let mut w = bob_writes("first");
    let kb = key(&w, 1);
    w.unlink(0, 1);
    let now = w.now;
    let sn = &mut w.nodes[0];
    let seq = sn
        .dev
        .send_text(now, &kb, b"slow link", &mut sn.st)
        .unwrap();
    assert!(w.run_until(secs(60), |w| status(w, 0, seq)
        == Status::Sending { attempt: 1 }));
    w.link(0, 1);
    assert!(w.run_until(secs(120), |w| matches!(
        status(w, 0, seq),
        Status::Delivered { .. }
    )));
}

#[test]
fn ping_and_status_from_the_screen_answer_the_screen_only() {
    let mut w = World::new(31);
    let a = w.add("Alice", |_| {});
    let b = w.add("Bob", |p| p.telemetry_base = 2);
    w.link(a, b);
    for (x, y) in [(a, b), (b, a)] {
        let now = w.now;
        w.node(x).send_self_advert(now, false).unwrap();
        let kx = key(&w, x);
        assert!(w.run_until(secs(10), |w| {
            w.nodes[y].dev.node.contacts.find(&kx).is_some()
        }));
    }
    let kb = key(&w, b);
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(9);
    let now = w.now;
    let tag = w.nodes[a]
        .dev
        .request(now, &kb, ReqKind::Telemetry, &mut rng)
        .unwrap();
    assert!(w.run_until(secs(30), |w| w.nodes[a].dev.request_reply(tag).is_some()));
    let r = w.nodes[a].dev.request_reply(tag).unwrap();
    assert!(r.ok && !r.data.is_empty() && r.rtt_ms > 0, "{r:?}");
    assert!(w.nodes[a].ui.contains(&UiEvent::Reply { tag }));
    assert_eq!(pushed(&w, a, 0x8B), 0, "no TELEMETRY_RESPONSE for the app");

    // No answer: the screen hears the timeout.
    w.unlink(a, b);
    let now = w.now;
    let tag = w.nodes[a]
        .dev
        .request(now, &kb, ReqKind::Status, &mut rng)
        .unwrap();
    assert!(w.run_until(secs(60), |w| w.nodes[a].dev.request_reply(tag).is_some()));
    assert!(!w.nodes[a].dev.request_reply(tag).unwrap().ok);
}

#[test]
fn settings_api_checks_like_the_companion_and_saves() {
    let mut w = World::new(41);
    let a = w.add("Alice", |_| {});
    let sn = &mut w.nodes[a];
    assert_eq!(
        sn.dev.set_tx_power(30, &mut sn.st),
        Err(mc_proto::codec::err::ILLEGAL_ARG)
    );
    assert_eq!(
        sn.dev.set_radio(869_618, 62_500, 13, 5, false, &mut sn.st),
        Err(mc_proto::codec::err::ILLEGAL_ARG)
    );
    assert_eq!(
        sn.dev.set_radio(869_525, 250_000, 11, 5, false, &mut sn.st),
        Ok(())
    );
    assert_eq!(sn.dev.poll_action(), Some(mc_node::Action::RadioChanged));
    let mut ui = sn.dev.node.prefs.ui;
    ui.compact = true;
    ui.tz_offset_min = 60;
    sn.dev.set_ui(ui, &mut sn.st);
    let saved =
        mc_node::persist::decode_prefs(sn.st.prefs.as_ref().unwrap(), &sn.dev.node.prefs).unwrap();
    assert_eq!(saved.ui, ui);
    assert_eq!(saved.radio.bw_hz, 250_000);
    let idx = sn.dev.add_hashtag_channel("#oxymesh", &mut sn.st).unwrap();
    assert_eq!(
        sn.dev.add_hashtag_channel("#oxymesh", &mut sn.st),
        Ok(idx),
        "same slot again"
    );
    assert_eq!(
        sn.dev.node.channels.0[usize::from(idx)].secret,
        mc_air::crypto::ChannelSecret::from_hashtag("#oxymesh")
    );
    sn.dev.remove_channel(idx, &mut sn.st).unwrap();
    assert!(sn.dev.node.channels.0[usize::from(idx)].is_empty());
}
