//! Protocole compagnon (`docs/spec/companion.md`) : commandes de l'app dans les deux modes de
//! décodage, et réponses de l'appareil.
//!
//! Oracle : jamais de panic ; toute trame acceptée en strict (commande ou réponse) se réécrit à
//! l'identique et passe aussi côté appareil avec le même sens ; toute commande acceptée côté
//! appareil se réécrit en une forme canonique que le strict relit à l'identique (ADR-0002) ; un
//! refus côté appareil est ERR 1 ou ERR 6, ou une trame que les porteurs ne remettent jamais.
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
