//! MeshCore over-the-air packets (`docs/spec/air.md`). The first byte selects: whole packet,
//! plaintext (direct message, channel, data, PATH) or advert data.
//!
//! Oracle: never panic; every accepted packet re-encodes identically; every payload split stays
//! within the packet; re-encoded advert data reads back the same.
#![no_main]

use libfuzzer_sys::fuzz_target;
use mc_air::payload::{Advert, GroupDataPlain, GroupTxtPlain, PathPlain, Payload, TxtPlain};
use mc_air::{AppData, Packet};

fuzz_target!(|data: &[u8]| {
    let Some((&which, bytes)) = data.split_first() else {
        return;
    };
    match which % 3 {
        0 => {
            if let Ok(p) = Packet::decode(bytes) {
                let mut out = [0u8; 255];
                let n = p.encode(&mut out).unwrap();
                assert_eq!(&out[..n], bytes);
                if let Ok(pl) = Payload::parse(p.header.ptype, p.payload) {
                    if let Payload::Advert(a) = pl {
                        let _ = a.verify();
                    }
                }
            }
        }
        1 => {
            let _ = TxtPlain::parse(bytes);
            let _ = GroupTxtPlain::parse(bytes);
            if let Ok(g) = GroupDataPlain::parse(bytes) {
                assert!(g.data.len() + 3 <= bytes.len());
            }
            if let Ok(pp) = PathPlain::parse(bytes) {
                assert_eq!(pp.path.len(), pp.path_len.byte_len());
            }
            let _ = Advert::parse(bytes);
        }
        _ => {
            if let Ok(ad) = AppData::decode(bytes) {
                let mut buf = [0u8; 32];
                let n = ad.encode(&mut buf).unwrap();
                let back = AppData::decode(&buf[..n]).unwrap();
                assert_eq!(back.kind, ad.kind);
                assert_eq!(back.location, ad.location);
                assert!(ad.name_str().starts_with(back.name_str()));
            }
        }
    }
});
