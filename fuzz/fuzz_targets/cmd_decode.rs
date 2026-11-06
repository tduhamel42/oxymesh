//! Companion protocol (`docs/spec/companion.md`): app commands in both decoding modes, and
//! device responses.
//!
//! Oracle: never panic; every frame accepted in strict mode (command or response) re-encodes
//! identically and also passes on the device side with the same meaning; every command accepted
//! on the device side re-encodes into a canonical form that strict mode reads back identically
//! (ADR-0002); a device-side rejection is ERR 1 or ERR 6, or a frame the carriers never hand
//! over.
#![no_main]

use libfuzzer_sys::fuzz_target;
use mc_proto::{Command, DeviceReject, FRAME_MAX, Response};

fuzz_target!(|data: &[u8]| {
    let mut out = [0u8; 256];
    if let Ok(c) = Command::decode(data) {
        let n = c.encode(&mut out).unwrap();
        assert_eq!(&out[..n], data);
        assert_eq!(Command::decode_device(data), Ok(c));
        let _ = c.to_string();
    }
    if let Ok(r) = Response::decode(data) {
        let n = r.encode(&mut out).unwrap();
        assert_eq!(&out[..n], data);
        let _ = r.to_string();
    }
    match Command::decode_device(data) {
        Ok(c) => {
            let n = c.encode(&mut out).unwrap();
            assert!(n <= FRAME_MAX);
            assert_eq!(Command::decode(&out[..n]), Ok(c));
            let _ = c.to_string();
        }
        Err(DeviceReject::Ignored) => assert!(data.is_empty() || data.len() > FRAME_MAX),
        Err(DeviceReject::Err(code)) => assert!(code == 1 || code == 6),
    }
});
