//! The companion layer end to end: two simulated devices driven by `mc-proto` frames, as the
//! official app would do.

use mc_core::Duration;
use mc_proto::codec::{APP_VER, fixed_str, to_fixed};
use mc_proto::{Command, Response};
use mc_sim::World;

fn enc(c: &Command<'_>) -> Vec<u8> {
    let mut b = [0u8; 256];
    let n = c.encode(&mut b).unwrap();
    b[..n].to_vec()
}

/// Sends a command and returns the frames (response, then any notifications).
fn ask(w: &mut World, i: usize, c: &Command<'_>) -> Vec<Vec<u8>> {
    w.app(i, &enc(c)).into_iter().map(|f| f.to_vec()).collect()
}

fn first(w: &mut World, i: usize, c: &Command<'_>) -> Vec<u8> {
    ask(w, i, c).into_iter().next().expect("a response")
}

fn pushed(w: &World, i: usize, pred: impl Fn(&Response<'_>) -> bool) -> bool {
    w.nodes[i]
        .app
        .iter()
        .any(|f| Response::decode(f).is_ok_and(|r| pred(&r)))
}

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

fn start(w: &mut World, i: usize) {
    let r = first(w, i, &Command::DeviceQuery { app_ver: APP_VER });
    match Response::decode(&r).unwrap() {
        Response::DeviceInfo(d) => {
            assert_eq!(d.fw_ver, 14);
            assert_eq!(d.max_contacts_half, 175);
            assert_eq!(d.max_channels, 40);
            assert_eq!(fixed_str(&d.version), b"v1.18.0-oxymesh");
        }
        other => panic!("expected DEVICE_INFO: {other:?}"),
    }
    let r = first(
        w,
        i,
        &Command::AppStart {
            reserved: [0; 7],
            app_name: b"test",
        },
    );
    assert!(matches!(
        Response::decode(&r).unwrap(),
        Response::SelfInfo(_)
    ));
}

#[test]
fn app_session_between_two_devices() {
    let mut w = World::new(10);
    let a = w.add("alice", |_| {});
    let b = w.add("bob", |_| {});
    w.link(a, b);
    start(&mut w, a);
    start(&mut w, b);

    // Bob's SELF_INFO: his key and name.
    let kb = *w.node(b).id.pub_key();
    let ka = *w.node(a).id.pub_key();
    match Response::decode(&first(
        &mut w,
        b,
        &Command::AppStart {
            reserved: [0; 7],
            app_name: b"x",
        },
    ))
    .unwrap()
    {
        Response::SelfInfo(s) => {
            assert_eq!(s.key, kb);
            assert_eq!(s.node_name, b"bob");
            assert_eq!((s.freq_khz, s.bw_hz, s.sf, s.cr), (869_618, 62_500, 8, 5));
        }
        r => panic!("{r:?}"),
    }

    // Zero-hop adverts: each pushes ADVERT and sees the other in GET_CONTACTS.
    assert_eq!(
        Response::decode(&first(
            &mut w,
            a,
            &Command::SendSelfAdvert { flood: Some(false) }
        ))
        .unwrap(),
        Response::Ok
    );
    assert!(w.run_until(secs(10), |w| pushed(
        w,
        b,
        |r| matches!(r, Response::Advert { key } if *key == ka)
    )));
    assert_eq!(
        Response::decode(&first(&mut w, b, &Command::SendSelfAdvert { flood: None })).unwrap(),
        Response::Ok
    );
    assert!(w.run_until(secs(10), |w| pushed(
        w,
        a,
        |r| matches!(r, Response::Advert { key } if *key == kb)
    )));
    let frames = ask(&mut w, a, &Command::GetContacts { since: None });
    let decoded: Vec<Response<'_>> = frames
        .iter()
        .map(|f| Response::decode(f).unwrap())
        .collect();
    assert_eq!(decoded[0], Response::ContactsStart { total: 1 });
    assert!(
        matches!(&decoded[1], Response::Contact(c) if c.key == kb && fixed_str(&c.name) == b"bob")
    );
    assert!(matches!(decoded[2], Response::EndOfContacts { .. }));

    // Direct message: SENT (flood), MSG_WAITING at Bob, SYNC, then SEND_CONFIRMED.
    let now = w.now;
    let ts = w.node(a).time(now);
    let sent = first(
        &mut w,
        a,
        &Command::SendTxtMsg {
            txt_type: 0,
            attempt: 0,
            timestamp: ts,
            prefix: kb[..6].try_into().unwrap(),
            text: "hi Bob — how are you?".as_bytes(),
        },
    );
    let Response::Sent {
        flood,
        ack,
        timeout_ms,
    } = Response::decode(&sent).unwrap()
    else {
        panic!("SENT")
    };
    assert!(flood);
    assert!(timeout_ms > 500);
    assert!(w.run_until(secs(30), |w| pushed(w, b, |r| matches!(
        r,
        Response::MsgWaiting
    ))));
    let m = first(&mut w, b, &Command::SyncNextMessage);
    match Response::decode(&m).unwrap() {
        Response::ContactMsg {
            prefix,
            text,
            timestamp,
            txt_type: 0,
            path_len,
            ..
        } => {
            assert_eq!(prefix, ka[..6]);
            assert_eq!(text, "hi Bob — how are you?".as_bytes());
            assert_eq!(timestamp, ts);
            assert_eq!(path_len, 0, "arrived by flood, 0 hops");
        }
        r => panic!("{r:?}"),
    }
    assert_eq!(
        Response::decode(&first(&mut w, b, &Command::SyncNextMessage)).unwrap(),
        Response::NoMoreMessages
    );
    assert!(w.run_until(secs(30), |w| pushed(
        w,
        a,
        |r| matches!(r, Response::SendConfirmed { ack: x, .. } if *x == ack)
    )));
    assert!(pushed(
        &w,
        a,
        |r| matches!(r, Response::PathUpdated { key } if *key == kb)
    ));

    // Private channel "#test" on both devices, channel message.
    let secret: [u8; 16] = mc_air::ChannelSecret::from_hashtag("#test").0[..16]
        .try_into()
        .unwrap();
    for i in [a, b] {
        assert_eq!(
            Response::decode(&first(
                &mut w,
                i,
                &Command::SetChannel {
                    idx: 1,
                    name: to_fixed::<32>(b"#test"),
                    secret
                }
            ))
            .unwrap(),
            Response::Ok
        );
    }
    let now = w.now;
    let ts = w.node(a).time(now);
    assert_eq!(
        Response::decode(&first(
            &mut w,
            a,
            &Command::SendChannelTxtMsg {
                txt_type: 0,
                idx: 1,
                timestamp: ts,
                text: b"hello channel"
            }
        ))
        .unwrap(),
        Response::Ok
    );
    w.nodes[b].app.clear();
    assert!(w.run_until(secs(10), |w| pushed(w, b, |r| matches!(
        r,
        Response::MsgWaiting
    ))));
    match Response::decode(&first(&mut w, b, &Command::SyncNextMessage)).unwrap() {
        Response::ChannelMsg { idx: 1, text, .. } => assert_eq!(text, b"alice: hello channel"),
        r => panic!("{r:?}"),
    }
    // Channel 0 is "Public".
    match Response::decode(&first(&mut w, a, &Command::GetChannel { idx: 0 })).unwrap() {
        Response::ChannelInfo { name, secret, .. } => {
            assert_eq!(fixed_str(&name), b"Public");
            assert_eq!(secret[..2], [0x8b, 0x33]);
        }
        r => panic!("{r:?}"),
    }
}

#[test]
fn error_codes_match_cpp() {
    let mut w = World::new(11);
    let a = w.add("alice", |_| {});
    start(&mut w, a);
    let err = |w: &mut World, c: &Command<'_>| match Response::decode(&first(w, a, c)).unwrap() {
        Response::Err(code) => code,
        r => panic!("expected ERR: {r:?}"),
    };
    // Unknown contact: ERR 2.
    assert_eq!(
        err(
            &mut w,
            &Command::SendTxtMsg {
                txt_type: 0,
                attempt: 0,
                timestamp: 1,
                prefix: [9; 6],
                text: b"x"
            }
        ),
        2
    );
    // The clock does not go backwards: ERR 6.
    assert_eq!(err(&mut w, &Command::SetDeviceTime { unix_s: 1 }), 6);
    // Channel out of range: ERR 2; channel text type ≠ 0: ERR 1.
    assert_eq!(
        err(
            &mut w,
            &Command::SendChannelTxtMsg {
                txt_type: 0,
                idx: 40,
                timestamp: 1,
                text: b"x"
            }
        ),
        2
    );
    assert_eq!(
        err(
            &mut w,
            &Command::SendChannelTxtMsg {
                txt_type: 1,
                idx: 0,
                timestamp: 1,
                text: b"x"
            }
        ),
        1
    );
    // Power out of range, invalid radio setting: ERR 6.
    assert_eq!(err(&mut w, &Command::SetRadioTxPower { dbm: 23 }), 6);
    assert_eq!(
        err(
            &mut w,
            &Command::SetRadioParams {
                freq_khz: 869_618,
                bw_hz: 62_500,
                sf: 13,
                cr: 5,
                repeat: None
            }
        ),
        6
    );
    // Repeat requested outside the allowed frequencies: ERR 6.
    assert_eq!(
        err(
            &mut w,
            &Command::SetRadioParams {
                freq_khz: 869_618,
                bw_hz: 62_500,
                sf: 8,
                cr: 8,
                repeat: Some(1)
            }
        ),
        6
    );
    // Frame too short for its command: ERR 1 (C++ length precondition).
    assert_eq!(
        Response::decode(&w.app(a, &[2, 0, 0])[0]).unwrap(),
        Response::Err(1)
    );
    // No message waiting.
    assert_eq!(
        Response::decode(&first(&mut w, a, &Command::SyncNextMessage)).unwrap(),
        Response::NoMoreMessages
    );
    // Private key: export then import of another key, the public key follows.
    let other = mc_sim::identity("carol");
    assert_eq!(
        Response::decode(&first(
            &mut w,
            a,
            &Command::ImportPrivateKey {
                key: *other.private_key()
            }
        ))
        .unwrap(),
        Response::Ok
    );
    match Response::decode(&first(&mut w, a, &Command::ExportPrivateKey)).unwrap() {
        Response::PrivateKey { key } => assert_eq!(key, *other.private_key()),
        r => panic!("{r:?}"),
    }
    assert_eq!(w.nodes[a].st.identity.unwrap().1, *other.pub_key());
}
