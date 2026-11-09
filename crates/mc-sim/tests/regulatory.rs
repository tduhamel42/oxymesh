//! The EU limits (`config/eu868.toml`, `docs/spec/air.md` §4.5): radiated power within the
//! sub-band's limit, counting the board's amplifier, and the sub-band's duty cycle on top of
//! the C++ airtime budget.

use mc_core::Duration;
use mc_node::device::FrontEnd;
use mc_proto::Command;
use mc_sim::World;

fn tune(w: &mut World, i: usize, freq_khz: u32, tx: i8) -> i8 {
    let p = &mut w.nodes[i].dev.node.prefs;
    p.radio.freq_khz = freq_khz;
    p.radio.bw_hz = 62_500;
    p.radio.sf = 8;
    p.radio.cr = 8;
    p.radio.tx_power_dbm = tx;
    w.nodes[i].dev.radio_tx_dbm()
}

#[test]
fn power_fits_the_sub_band() {
    let mut w = World::new(31);
    let a = w.add("alice", |_| {});
    // g3 (869.4-869.65 MHz, 500 mW = 26.99 dBm): the M5 at 22 dBm is fine.
    assert_eq!(tune(&mut w, a, 869_618, 22), 22);
    // With an amplifier (Heltec V4): 16 dBm + 10 dB is the most that fits.
    w.nodes[a].dev.info.front_end = FrontEnd::PaLna;
    assert_eq!(tune(&mut w, a, 869_618, 22), 16);
    assert_eq!(tune(&mut w, a, 869_618, 10), 10);
    // g1 (868.0-868.6 MHz, 25 mW = 13.98 dBm).
    w.nodes[a].dev.info.front_end = FrontEnd::None;
    assert_eq!(tune(&mut w, a, 868_100, 22), 13);
    // Outside the EU profile: the setting as is.
    assert_eq!(tune(&mut w, a, 915_000, 22), 22);
    // Between two sub-bands: left to the C++ budget, setting as is.
    assert_eq!(tune(&mut w, a, 869_300, 20), 20);
}

/// Adverts sent in a minute, one asked for every 2 s.
fn adverts_in_a_minute(freq_khz: u32) -> (usize, Duration) {
    let mut w = World::new(32);
    let a = w.add("alice", |p| {
        p.radio.freq_khz = freq_khz;
        p.radio.bw_hz = 62_500;
        p.radio.sf = 8;
        p.radio.cr = 8;
        // The C++ budget out of the way (af 0 = 100 %).
        p.airtime_factor_milli = 0;
    });
    w.nodes[a].dev.radio_tx_dbm();
    let mut b = [0u8; 64];
    let n = Command::SendSelfAdvert { flood: Some(false) }
        .encode(&mut b)
        .unwrap();
    for _ in 0..30 {
        w.app(a, &b[..n]);
        w.run(Duration::from_secs(2));
    }
    let air = w.nodes[a]
        .dev
        .node
        .prefs
        .radio
        .lora()
        .airtime(w.log[0].2.len());
    (w.log.len(), air)
}

#[test]
fn duty_cycle_of_the_sub_band_holds_frames_back() {
    // g2 (868.7-869.2 MHz): 0.1 %, 3.6 s an hour, less the 1 % margin.
    let (sent, air) = adverts_in_a_minute(868_950);
    let used = air.as_millis() * sent as u64;
    assert!(used <= 3564, "{sent} adverts, {used} ms");
    assert!(
        used + air.as_millis() > 3564,
        "{sent} adverts, {used} ms: one more would fit"
    );
    // Outside the profile, nothing but the C++ budget: all 30 go.
    assert_eq!(adverts_in_a_minute(915_000).0, 30);
}
