//! The GPS as the C++ companion uses it (H2): its position goes to adverts, `SELF_INFO` and
//! telemetry every `gps_interval` seconds; `SET_ADVERT_LATLON` wins until the next update; the
//! clock follows the receiver. Positions here are made up.

use mc_core::Duration;
use mc_node::device::{Action, GpsFix};
use mc_proto::codec::to_fixed;
use mc_proto::{Command, Response};
use mc_sim::World;

fn enc(c: &Command<'_>) -> Vec<u8> {
    let mut b = [0u8; 256];
    let n = c.encode(&mut b).unwrap();
    b[..n].to_vec()
}

fn ask(w: &mut World, i: usize, c: &Command<'_>) -> Vec<u8> {
    w.app(i, &enc(c)).into_iter().next().unwrap().to_vec()
}

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

const FIX: GpsFix = GpsFix {
    lat: 45_123_456,
    lon: -5_654_321,
    alt_mm: 212_345,
    satellites: 7,
};

/// Alice has a GPS, shares her position in adverts and telemetry; Bob knows her.
fn pair() -> World {
    let mut w = World::new(31);
    let a = w.add("Alice", |p| {
        p.advert_loc_policy = 1;
        p.telemetry_base = 2;
        p.telemetry_loc = 2;
    });
    let b = w.add("Bob", |_| {});
    w.link(a, b);
    let sn = &mut w.nodes[a];
    sn.dev.info.has_gps = true;
    sn.dev.set_gps(true, &mut sn.st).unwrap();
    sn.dev.mcu_temp_deci = Some(215);
    sn.dev.battery_mv = 3_900;
    w
}

fn self_pos(w: &mut World) -> (i32, i32) {
    let r = ask(
        w,
        0,
        &Command::AppStart {
            reserved: [0; 7],
            app_name: b"t",
        },
    );
    match Response::decode(&r).unwrap() {
        Response::SelfInfo(s) => (s.lat, s.lon),
        other => panic!("{other:?}"),
    }
}

/// CayenneLPP GPS record of `FIX`, channel 1.
fn lpp_gps() -> Vec<u8> {
    let b3 = |x: i32| x.to_be_bytes()[1..].to_vec();
    let mut g = vec![1, 136];
    g.extend(b3(451_234));
    g.extend(b3(-56_543));
    g.extend(b3(21_234));
    g
}

#[test]
fn the_fix_goes_to_self_info_adverts_and_telemetry() {
    let mut w = pair();
    let stored = (w.nodes[0].dev.node.prefs.lat, w.nodes[0].dev.node.prefs.lon);
    assert_eq!(self_pos(&mut w), stored, "no fix yet");
    w.nodes[0].dev.report_gps(Some(FIX));
    w.run(secs(2));
    assert_eq!(self_pos(&mut w), (FIX.lat, FIX.lon));

    // The app's own telemetry: voltage, temperature, then the position.
    let r = ask(
        &mut w,
        0,
        &Command::SendTelemetryReq {
            reserved: [0; 3],
            key: None,
        },
    );
    let mut want = vec![1, 116, 1, 134, 1, 103, 0, 215];
    want.extend(lpp_gps());
    match Response::decode(&r).unwrap() {
        Response::TelemetryResponse { lpp, .. } => assert_eq!(lpp, &want[..]),
        other => panic!("{other:?}"),
    }

    // Alice's advert carries the GPS position.
    let now = w.now;
    w.node(0).send_self_advert(now, false).unwrap();
    let ka = *w.node(0).id.pub_key();
    assert!(w.run_until(secs(10), |w| {
        let c = &w.nodes[1].dev.node.contacts;
        c.find(&ka)
            .and_then(|s| c.get(s))
            .is_some_and(|c| (c.lat, c.lon) == (FIX.lat, FIX.lon))
    }));

    // Bob asks for her telemetry: voltage, the position, then temperature.
    let now = w.now;
    w.node(1).send_self_advert(now, false).unwrap();
    w.run(secs(5));
    w.nodes[1].app.clear();
    let r = ask(
        &mut w,
        1,
        &Command::SendTelemetryReq {
            reserved: [0; 3],
            key: Some(ka),
        },
    );
    assert!(matches!(
        Response::decode(&r).unwrap(),
        Response::Sent { .. }
    ));
    let mut want = vec![1, 116, 1, 134];
    want.extend(lpp_gps());
    want.extend([1, 103, 0, 215]);
    assert!(w.run_until(secs(20), |w| {
        w.nodes[1].app.iter().any(|f| {
            // Then the zeros of the last AES block, as from the C++.
            matches!(Response::decode(f), Ok(Response::TelemetryResponse { lpp, .. })
                if lpp.starts_with(&want) && lpp[want.len()..].iter().all(|&b| b == 0))
        })
    }));

    // GPS off: no position in telemetry.
    let sn = &mut w.nodes[0];
    sn.dev.set_gps(false, &mut sn.st).unwrap();
    let r = ask(
        &mut w,
        0,
        &Command::SendTelemetryReq {
            reserved: [0; 3],
            key: None,
        },
    );
    match Response::decode(&r).unwrap() {
        Response::TelemetryResponse { lpp, .. } => {
            assert_eq!(lpp, &[1, 116, 1, 134, 1, 103, 0, 215][..]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn manual_position_until_the_next_update_and_the_interval_is_saved() {
    let mut w = pair();
    let r = ask(
        &mut w,
        0,
        &Command::SetCustomVar {
            var: b"gps_interval:60",
        },
    );
    assert!(matches!(Response::decode(&r).unwrap(), Response::Ok));
    let saved = mc_node::persist::decode_prefs(
        w.nodes[0].st.prefs.as_ref().unwrap(),
        &w.nodes[0].dev.node.prefs,
    )
    .unwrap();
    assert_eq!(saved.gps_interval, 60);

    w.nodes[0].dev.report_gps(Some(FIX));
    w.run(secs(61));
    assert_eq!(self_pos(&mut w), (FIX.lat, FIX.lon));
    // The app sets a position: it wins until the GPS's next turn.
    let r = ask(
        &mut w,
        0,
        &Command::SetAdvertLatLon {
            lat: 1_000_000,
            lon: 2_000_000,
            alt: None,
        },
    );
    assert!(matches!(Response::decode(&r).unwrap(), Response::Ok));
    assert_eq!(self_pos(&mut w), (1_000_000, 2_000_000));
    w.run(secs(61));
    assert_eq!(self_pos(&mut w), (FIX.lat, FIX.lon));
    // A new fix waits for the interval.
    w.nodes[0].dev.report_gps(Some(GpsFix { lat: 0, ..FIX }));
    w.run(secs(2));
    assert_eq!(self_pos(&mut w).0, FIX.lat);
    w.run(secs(60));
    assert_eq!(self_pos(&mut w).0, 0);
}

#[test]
fn setting_the_clock_asks_for_the_clock_chip() {
    let mut w = pair();
    let t = mc_sim::EPOCH + 10_000;
    let r = ask(&mut w, 0, &Command::SetDeviceTime { unix_s: t });
    assert!(matches!(Response::decode(&r).unwrap(), Response::Ok));
    assert_eq!(w.nodes[0].dev.poll_action(), Some(Action::ClockSet));
    let now = w.now;
    w.nodes[0].dev.set_time(now, t + 5);
    assert_eq!(w.nodes[0].dev.poll_action(), Some(Action::ClockSet));
    assert_eq!(w.nodes[0].dev.node.time(now), t + 5);
    let _ = to_fixed::<4>(b"");
}
