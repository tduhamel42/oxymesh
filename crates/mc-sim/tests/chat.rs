//! End-to-end scenarios between `mc-node` instances over the simulated radio.

use mc_core::Duration;
use mc_node::contacts::PATH_UNKNOWN;
use mc_node::{Event, Sent};
use mc_sim::{EPOCH, World};

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

fn has(w: &World, i: usize, f: impl Fn(&Event) -> bool) -> bool {
    w.nodes[i].events.iter().any(f)
}

/// Two nodes advertise and become each other's contacts.
fn meet(w: &mut World, a: usize, b: usize, flood: bool) {
    let now = w.now;
    w.node(a).send_self_advert(now, flood).unwrap();
    let ka = *w.node(a).id.pub_key();
    assert!(
        w.run_until(secs(10), |w| has(
            w,
            b,
            |e| matches!(e, Event::Advert { key, .. } if *key == ka)
        )),
        "advert from {a} not received by {b}"
    );
    let now = w.now;
    w.node(b).send_self_advert(now, flood).unwrap();
    let kb = *w.node(b).id.pub_key();
    assert!(
        w.run_until(secs(10), |w| has(
            w,
            a,
            |e| matches!(e, Event::Advert { key, .. } if *key == kb)
        )),
        "advert from {b} not received by {a}"
    );
}

fn send(w: &mut World, from: usize, to: usize, text: &str) -> Sent {
    let now = w.now;
    let ts = w.node(from).time(now);
    let prefix: [u8; 6] = w.node(to).id.pub_key()[..6].try_into().unwrap();
    w.node(from)
        .send_text(now, &prefix, 0, 0, ts, text.as_bytes())
        .unwrap()
}

fn delivered(w: &mut World, from: usize, to: usize, text: &str, sent: Sent) {
    let kf = *w.node(from).id.pub_key();
    let t = text.as_bytes().to_vec();
    assert!(
        w.run_until(secs(30), |w| has(w, to, |e| matches!(e, Event::ContactMsg { key, text, txt_type: 0, .. } if *key == kf && **text == t[..]))),
        "message \"{text}\" not received"
    );
    let ack = sent.ack.to_le_bytes();
    assert!(
        w.run_until(secs(30), |w| has(
            w,
            from,
            |e| matches!(e, Event::SendConfirmed { ack: a, .. } if *a == ack)
        )),
        "ack for \"{text}\" not received"
    );
}

#[test]
fn advert_message_ack_and_path_learning() {
    let mut w = World::new(1);
    let a = w.add("alice", |_| {});
    let b = w.add("bob", |_| {});
    w.link(a, b);
    meet(&mut w, a, b, false);
    let kb = *w.node(b).id.pub_key();
    let c = w.node(a).contacts.find(&kb).unwrap();
    let bob = w.node(a).contacts.get(c).unwrap();
    assert_eq!(mc_node::contacts::name_str(&bob.name), b"bob");
    assert_eq!(bob.out_path_len, PATH_UNKNOWN);

    // First message: flood, return PATH carrying the ack, path learned (0 hops).
    let s1 = send(&mut w, a, b, "hello bob");
    assert!(s1.flood);
    delivered(&mut w, a, b, "hello bob", s1);
    assert_eq!(w.node(a).contacts.get(c).unwrap().out_path_len, 0);
    // The reciprocal PATH (500 ms) teaches Bob the path to Alice.
    let ka = *w.node(a).id.pub_key();
    assert!(w.run_until(secs(10), |w| {
        let n = &w.nodes[b].dev.node;
        n.contacts
            .find(&ka)
            .and_then(|s| n.contacts.get(s))
            .is_some_and(|c| c.out_path_len == 0)
    }));

    // Second message: direct, direct ack.
    let s2 = send(&mut w, a, b, "second one, direct");
    assert!(!s2.flood);
    delivered(&mut w, a, b, "second one, direct", s2);
    // Bob's reply, also direct.
    let s3 = send(&mut w, b, a, "reply");
    assert!(!s3.flood);
    delivered(&mut w, b, a, "reply", s3);
}

#[test]
fn public_channel() {
    let mut w = World::new(2);
    let a = w.add("alice", |_| {});
    let b = w.add("bob", |_| {});
    w.link(a, b);
    let now = w.now;
    w.node(a)
        .send_channel_text(now, 0, EPOCH, "hello everyone — see you".as_bytes())
        .unwrap();
    let want = "alice: hello everyone — see you".as_bytes().to_vec();
    assert!(w.run_until(secs(10), |w| has(
        w,
        b,
        |e| matches!(e, Event::ChannelMsg { idx: 0, text, .. } if **text == want[..])
    )));
}

#[test]
fn one_hop_through_a_relay() {
    let mut w = World::new(3);
    let a = w.add("alice", |_| {});
    let r = w.add("relay", |p| p.repeat = true);
    let b = w.add("bob", |_| {});
    w.link(a, r);
    w.link(r, b);
    meet(&mut w, a, b, true);
    let s1 = send(&mut w, a, b, "through the relay");
    assert!(s1.flood);
    delivered(&mut w, a, b, "through the relay", s1);
    // Alice learned a one-hop path: the relay's hash.
    let kb = *w.node(b).id.pub_key();
    let rh = w.node(r).id.hash();
    let ab = {
        let n = w.node(a);
        let s = n.contacts.find(&kb).unwrap();
        n.contacts.get(s).unwrap().clone()
    };
    assert_eq!(ab.out_path_len, 1);
    assert_eq!(ab.out_path()[0], rh);
    w.run(secs(5));
    let s2 = send(&mut w, a, b, "direct via the relay");
    assert!(!s2.flood);
    delivered(&mut w, a, b, "direct via the relay", s2);
}

#[test]
fn duplicate_frames_are_ignored() {
    let mut w = World::new(4);
    let a = w.add("alice", |_| {});
    let b = w.add("bob", |_| {});
    w.link(a, b);
    meet(&mut w, a, b, false);
    let s = send(&mut w, a, b, "only once");
    delivered(&mut w, a, b, "only once", s);
    // Replay the message frame: Bob does not deliver it twice.
    let (_, _, raw) = w
        .log
        .iter()
        .find(|(_, from, raw)| *from == a && raw[0] >> 2 & 0xF == 2)
        .cloned()
        .unwrap();
    let now = w.now;
    let sn = &mut w.nodes[b];
    let mut rng = rand_chacha::ChaCha20Rng::from_seed([9; 32]);
    use rand_core::SeedableRng;
    sn.dev.on_rx(now, &raw, 40, -90, &mut rng, &mut sn.st);
    w.run(secs(2));
    let n = w.nodes[b]
        .events
        .iter()
        .filter(|e| matches!(e, Event::ContactMsg { text, .. } if **text == b"only once"[..]))
        .count();
    assert_eq!(n, 1);
}
