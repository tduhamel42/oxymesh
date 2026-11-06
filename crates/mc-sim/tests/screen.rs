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
