//! The device console (`RUN_CLI_COMMAND`, and console commands over the air), answered with the
//! C++ companion's texts (`docs/spec/companion.md` §6).

use mc_core::Duration;
use mc_node::device::{Action, FrontEnd, REMOTE_CLI};
use mc_proto::codec::APP_VER;
use mc_proto::{Command, Response};
use mc_sim::World;

fn enc(c: &Command<'_>) -> Vec<u8> {
    let mut b = [0u8; 256];
    let n = c.encode(&mut b).unwrap();
    b[..n].to_vec()
}

fn ask(w: &mut World, i: usize, c: &Command<'_>) -> Vec<Vec<u8>> {
    w.app(i, &enc(c)).into_iter().map(|f| f.to_vec()).collect()
}

/// The console's reply, or `None` when the device stays silent.
fn cli(w: &mut World, i: usize, text: &str) -> Option<String> {
    let frames = ask(
        w,
        i,
        &Command::RunCliCommand {
            cmd: text.as_bytes(),
        },
    );
    frames.iter().find_map(|f| match Response::decode(f) {
        Ok(Response::CliReply { text }) => Some(String::from_utf8(text.to_vec()).unwrap()),
        _ => None,
    })
}

fn actions(w: &mut World, i: usize) -> Vec<Action> {
    let mut v = Vec::new();
    while let Some(a) = w.nodes[i].dev.poll_action() {
        v.push(a);
    }
    v
}

fn start(w: &mut World, i: usize) {
    ask(w, i, &Command::DeviceQuery { app_ver: APP_VER });
    ask(
        w,
        i,
        &Command::AppStart {
            reserved: [0; 7],
            app_name: b"test",
        },
    );
}

#[test]
fn console_replies_like_the_cpp() {
    let mut w = World::new(21);
    let a = w.add("alice", |p| {
        p.radio.freq_khz = 869_618;
        p.radio.bw_hz = 62_500;
        p.radio.sf = 8;
        p.radio.cr = 8;
    });
    start(&mut w, a);
    actions(&mut w, a);

    let c = |w: &mut World, t: &str| cli(w, a, t);
    assert_eq!(
        c(&mut w, "get radio").as_deref(),
        Some("> 869.6179809,62.5,8,8")
    );
    assert_eq!(c(&mut w, "   get name").as_deref(), Some("> alice"));
    assert_eq!(c(&mut w, "ab|get name").as_deref(), Some("ab|> alice"));
    // The prefix needs more than 4 characters in all.
    assert_eq!(c(&mut w, "ab|x").as_deref(), Some("Unknown command"));
    assert_eq!(c(&mut w, "frobnicate").as_deref(), Some("Unknown command"));
    assert_eq!(
        c(&mut w, "ver").as_deref(),
        Some("v1.18.0-oxymesh (Build: 06-Nov-2026)")
    );
    assert_eq!(c(&mut w, "board").as_deref(), Some("oxymesh sim"));
    // The C++ companion has no `clock` command.
    assert_eq!(c(&mut w, "clock").as_deref(), Some("Unknown command"));

    // Names: the C++'s forbidden characters.
    assert_eq!(
        c(&mut w, "set name a,b").as_deref(),
        Some("Error, bad chars")
    );
    assert_eq!(c(&mut w, "set name Alice M5").as_deref(), Some("OK"));
    assert_eq!(c(&mut w, "get name").as_deref(), Some("> Alice M5"));

    // TX power: applied at once when the radio can take it.
    actions(&mut w, a);
    assert_eq!(c(&mut w, "set tx 14").as_deref(), Some("OK"));
    assert_eq!(actions(&mut w, a), vec![Action::RadioChanged]);
    assert_eq!(c(&mut w, "set tx 40").as_deref(), Some("OK"));
    assert!(
        actions(&mut w, a).is_empty(),
        "out of range: saved, not applied"
    );
    assert_eq!(c(&mut w, "get tx").as_deref(), Some("> 40"));

    // Radio parameters: saved, applied at the next start.
    assert_eq!(
        c(&mut w, "set radio 869.525,250,11,5").as_deref(),
        Some("OK - reboot to apply")
    );
    assert!(actions(&mut w, a).is_empty());
    assert_eq!(w.node(a).prefs.radio.freq_khz, 869_525);

    // Time zone in hours.
    assert_eq!(c(&mut w, "set tz.offset 2").as_deref(), Some("OK"));
    assert_eq!(w.nodes[a].dev.node.prefs.ui.tz_offset_min, 120);
    assert_eq!(c(&mut w, "get tz.offset").as_deref(), Some("> 2"));
    assert_eq!(
        c(&mut w, "set tz.offset 15").as_deref(),
        Some("Error, must be from -12 to +14")
    );

    assert_eq!(
        c(&mut w, "set pin 42").as_deref(),
        Some("> pin is now 000042")
    );
    assert_eq!(w.node(a).prefs.ble_pin, 42);
    actions(&mut w, a);

    // Reboot and power off: no reply, like the C++ that has already gone.
    assert_eq!(c(&mut w, "reboot"), None);
    assert_eq!(actions(&mut w, a), vec![Action::Reboot]);
    assert_eq!(c(&mut w, "shutdown"), None);
    assert_eq!(actions(&mut w, a), vec![Action::PowerOff]);

    // Front-end LNA: only on boards that have one.
    assert_eq!(
        c(&mut w, "get radio.fem.rxgain").as_deref(),
        Some("Unknown command")
    );
    w.nodes[a].dev.info.front_end = FrontEnd::Pa;
    assert_eq!(
        c(&mut w, "set radio.fem.rxgain on").as_deref(),
        Some("Error: unsupported")
    );
    w.nodes[a].dev.info.front_end = FrontEnd::PaLna;
    assert_eq!(c(&mut w, "get radio.fem.rxgain").as_deref(), Some("> on"));
    assert_eq!(
        c(&mut w, "set radio.fem.rxgain off").as_deref(),
        Some("OK - LoRa FEM RX gain off")
    );
    assert_eq!(
        c(&mut w, "set radio.fem.rxgain maybe").as_deref(),
        Some("Error: state must be on or off")
    );
    assert!(!w.node(a).prefs.fem_rxgain);
}

#[test]
fn settings_from_the_console_survive_a_restart() {
    let mut w = World::new(22);
    let a = w.add("alice", |_| {});
    start(&mut w, a);
    assert_eq!(
        cli(&mut w, a, "set agc.reset.interval 60").as_deref(),
        Some("OK - interval rounded to 60")
    );
    assert_eq!(cli(&mut w, a, "set int.thresh 12").as_deref(), Some("OK"));
    assert_eq!(cli(&mut w, a, "set cad on").as_deref(), Some("OK"));
    assert_eq!(
        cli(&mut w, a, "set radio.rxgain off").as_deref(),
        Some("OK")
    );
    // What the firmware reads back at the next start.
    let rec = w.nodes[a].st.prefs.clone().unwrap();
    let defaults = mc_node::Prefs::defaults(&[0; 32]);
    let p = mc_node::persist::decode_prefs(&rec, &defaults).unwrap();
    assert_eq!(
        (
            p.agc_reset_interval,
            p.interference_threshold,
            p.cad_enabled,
            p.rx_boosted_gain
        ),
        (15, 12, true, false)
    );
}

#[test]
fn console_commands_over_the_air() {
    let mut w = World::new(23);
    let a = w.add("alice", |_| {});
    let b = w.add("bob", |_| {});
    w.link(a, b);
    start(&mut w, a);
    start(&mut w, b);
    ask(&mut w, a, &Command::SendSelfAdvert { flood: Some(false) });
    ask(&mut w, b, &Command::SendSelfAdvert { flood: Some(false) });
    w.run(Duration::from_secs(10));
    let kb = *w.node(b).id.pub_key();
    let ka = *w.node(a).id.pub_key();
    let send = |w: &mut World, text: &[u8]| {
        ask(
            w,
            a,
            &Command::SendTxtMsg {
                txt_type: 3,
                attempt: 0,
                timestamp: 0,
                prefix: kb[..6].try_into().unwrap(),
                text,
            },
        );
    };

    // Not allowed: the command goes to Bob's app as is.
    send(&mut w, b"get name");
    assert!(w.run_until(Duration::from_secs(20), |w| {
        w.nodes[b]
            .app
            .iter()
            .any(|f| matches!(Response::decode(f), Ok(Response::MsgWaiting)))
    }));
    let m = ask(&mut w, b, &Command::SyncNextMessage);
    assert!(matches!(
        Response::decode(&m[0]),
        Ok(Response::ContactMsg {
            txt_type: 3,
            text: b"get name",
            ..
        })
    ));

    // Allowed (flag 0x10 on Bob's contact for Alice): Bob answers, Alice's app gets CLI data.
    let slot = w.node(b).contacts.find(&ka).unwrap();
    w.node(b).contacts.get_mut(slot).unwrap().flags |= REMOTE_CLI;
    w.nodes[a].app.clear();
    send(&mut w, b"get name");
    assert!(w.run_until(Duration::from_secs(20), |w| {
        w.nodes[a]
            .app
            .iter()
            .any(|f| matches!(Response::decode(f), Ok(Response::MsgWaiting)))
    }));
    let m = ask(&mut w, a, &Command::SyncNextMessage);
    match Response::decode(&m[0]) {
        Ok(Response::ContactMsg {
            txt_type: 1,
            text,
            prefix,
            ..
        }) => {
            assert_eq!(text, b"> bob");
            assert_eq!(prefix, kb[..6]);
        }
        r => panic!("{r:?}"),
    }
    // Nothing for Bob's app this time.
    let m = ask(&mut w, b, &Command::SyncNextMessage);
    assert!(matches!(
        Response::decode(&m[0]),
        Ok(Response::NoMoreMessages)
    ));
}
