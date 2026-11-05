//! Vecteurs de `testdata/proto/vectors.toml` (générés par `gen.mjs`, recoupés avec meshcore.js ;
//! repris de loranet) et propriétés du codec et du tramage USB.

use mc_proto::{Command, DecodeError, FROM_DEVICE, Response, TO_DEVICE, UsbDeframer, usb_frame};
use proptest::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct File {
    command: Vec<Frame>,
    response: Vec<Frame>,
}

#[derive(Deserialize)]
struct Frame {
    name: String,
    hex: String,
    expect: String,
    dump: Option<String>,
}

fn load() -> File {
    toml::from_str(include_str!("../../../testdata/proto/vectors.toml")).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn kind(e: DecodeError) -> String {
    format!("err:{e:?}")
}

#[test]
fn command_vectors() {
    for v in load().command {
        let bytes = unhex(&v.hex);
        match Command::decode(&bytes) {
            Ok(c) => {
                assert_eq!(v.expect, "ok", "{} accepté", v.name);
                assert_eq!(Some(c.to_string()), v.dump, "{}", v.name);
                let mut out = [0u8; 256];
                let n = c.encode(&mut out).unwrap();
                assert_eq!(hex(&out[..n]), v.hex, "{} réencodé", v.name);
            }
            Err(e) => assert_eq!(kind(e), v.expect, "{}", v.name),
        }
    }
}

#[test]
fn response_vectors() {
    for v in load().response {
        let bytes = unhex(&v.hex);
        match Response::decode(&bytes) {
            Ok(r) => {
                assert_eq!(v.expect, "ok", "{} accepté", v.name);
                assert_eq!(Some(r.to_string()), v.dump, "{}", v.name);
                let mut out = [0u8; 256];
                let n = r.encode(&mut out).unwrap();
                assert_eq!(hex(&out[..n]), v.hex, "{} réencodé", v.name);
            }
            Err(e) => assert_eq!(kind(e), v.expect, "{}", v.name),
        }
    }
}

proptest! {
    #[test]
    fn any_accepted_frame_is_canonical(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        let mut out = [0u8; 256];
        if let Ok(r) = Response::decode(&bytes) {
            let n = r.encode(&mut out).unwrap();
            prop_assert_eq!(&out[..n], &bytes[..]);
        }
        if let Ok(c) = Command::decode(&bytes) {
            let n = c.encode(&mut out).unwrap();
            prop_assert_eq!(&out[..n], &bytes[..]);
        }
    }

    #[test]
    fn usb_frames_survive_noise(
        noise in prop::collection::vec(any::<u8>().prop_filter("pas de début", |b| *b != FROM_DEVICE), 0..20),
        frames in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..=176), 1..5),
    ) {
        let mut stream = noise.clone();
        for f in &frames {
            let mut buf = [0u8; 180];
            let n = usb_frame(FROM_DEVICE, f, &mut buf).unwrap();
            stream.extend_from_slice(&buf[..n]);
        }
        let mut d = UsbDeframer::new(FROM_DEVICE);
        let mut got = Vec::new();
        for b in stream {
            if let Some(f) = d.push(b) {
                got.push(f.to_vec());
            }
        }
        prop_assert_eq!(got, frames);
    }
}

#[test]
fn usb_frame_refuses_bad_sizes() {
    let mut buf = [0u8; 200];
    assert!(usb_frame(TO_DEVICE, &[], &mut buf).is_err());
    assert!(usb_frame(TO_DEVICE, &[0; 177], &mut buf).is_err());
    assert_eq!(usb_frame(TO_DEVICE, &[0x0a], &mut buf), Ok(4));
    assert_eq!(&buf[..4], &[b'<', 1, 0, 0x0a]);
}
