//! GPS NMEA sentences (`mc-nmea`): bytes straight from the receiver's UART.
//!
//! Oracle: never panic; positions stay on the globe, times and dates in range; the clock sync
//! never yields a time the state cannot justify.
#![no_main]

use libfuzzer_sys::fuzz_target;
use mc_nmea::{Gps, Lines, Sentence, TimeSync, parse};

fuzz_target!(|data: &[u8]| {
    let _ = parse(data);
    let mut lines = Lines::new();
    let mut gps = Gps::default();
    let mut sync = TimeSync::new();
    for (i, &b) in data.iter().enumerate() {
        if let Some(s) = lines.push(b) {
            if let Sentence::Rmc { pos: Some(p), .. } | Sentence::Gga { pos: Some(p), .. } = s {
                assert!(p.lat.abs() <= 90_000_000 && p.lon.abs() <= 180_000_000);
            }
            gps.apply(&s);
            if let Some(t) = gps.time {
                assert!(t.hour < 24 && t.minute < 60 && t.second < 60);
            }
        }
        if let Some(t) = sync.poll(i as u64 * 1000, &gps) {
            assert!(gps.valid);
            assert_eq!(gps.unix(), Some(t));
        }
    }
});
