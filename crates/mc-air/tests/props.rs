//! Propriétés : canonicité du décodeur de trames, aller-retour des constructeurs, crypto.

use mc_air::crypto::{self, ChannelSecret, LocalIdentity};
use mc_air::payload::{self, GroupDataPlain, Payload, TxtPlain};
use mc_air::{AppData, Packet, PacketBuf, PathLen, PayloadType, Route};
use proptest::prelude::*;

proptest! {
    /// Toute trame acceptée se réécrit à l'identique, et son découpage ne panique pas.
    #[test]
    fn decode_is_canonical(raw in proptest::collection::vec(any::<u8>(), 0..300)) {
        if let Ok(p) = Packet::decode(&raw) {
            let mut out = [0u8; 255];
            let n = p.encode(&mut out).unwrap();
            prop_assert_eq!(&out[..n], &raw[..]);
            let _ = Payload::parse(p.header.ptype, p.payload);
        }
    }

    /// Une trame construite se décode en elle-même.
    #[test]
    fn built_packets_roundtrip(
        route in 0u8..4,
        ptype in 0u8..16,
        hash_size in 1u8..4,
        hops in 0u8..22,
        payload in proptest::collection::vec(any::<u8>(), 0..185),
        tc in any::<[u16; 2]>(),
    ) {
        let route = Route::from_bits(route);
        let mut b = PacketBuf::new(route, PayloadType::new(ptype));
        if route.has_transport_codes() {
            b.transport = Some(tc);
        }
        let pl = PathLen::new(hash_size, hops).unwrap();
        let path: Vec<u8> = (0..pl.byte_len()).map(|i| i as u8).collect();
        b.set_path(pl, &path).unwrap();
        b.set_payload(&payload).unwrap();
        let mut out = [0u8; 255];
        match b.encode(&mut out) {
            Ok(n) => {
                let d = Packet::decode(&out[..n]).unwrap();
                prop_assert_eq!(d, b.as_packet());
                prop_assert_eq!(d.hash(), b.hash());
            }
            Err(e) => prop_assert_eq!(e, mc_air::Error::FrameTooLong),
        }
    }

    /// seal puis open redonne le clair suivi de zéros ; tout autre secret échoue presque
    /// toujours (MAC de 2 octets : une chance sur 65 536 de passer).
    #[test]
    fn seal_open(secret in any::<[u8; 32]>(), plain in proptest::collection::vec(any::<u8>(), 1..170)) {
        let mut sealed = [0u8; 200];
        let n = crypto::seal(&secret, &plain, &mut sealed).unwrap();
        prop_assert_eq!(n, crypto::sealed_len(plain.len()));
        let mut out = [0u8; 200];
        let opened = crypto::open(&secret, &sealed[..n], &mut out).unwrap();
        prop_assert_eq!(&opened[..plain.len()], &plain[..]);
        prop_assert!(opened[plain.len()..].iter().all(|&b| b == 0));
    }

    /// Message direct de bout en bout entre deux identités quelconques.
    #[test]
    fn direct_message_end_to_end(
        sa in any::<[u8; 32]>(),
        sb in any::<[u8; 32]>(),
        ts in any::<u32>(),
        attempt in 0u8..8,
        text in "[a-zA-Z0-9 éàçù!?.,'-]{1,50}",
    ) {
        let a = LocalIdentity::from_seed(&sa);
        let b = LocalIdentity::from_seed(&sb);
        let s_ab = a.shared_secret(b.pub_key()).unwrap();
        prop_assert_eq!(s_ab, b.shared_secret(a.pub_key()).unwrap());
        let mut plain = [0u8; 200];
        let (n, ack) = payload::txt_plain(ts, 0, attempt, text.as_bytes(), a.pub_key(), &mut plain).unwrap();
        let pkt = payload::datagram(PayloadType::TXT_MSG, b.hash(), a.hash(), &s_ab, &plain[..n]).unwrap();
        let Payload::Addressed { sealed, .. } = Payload::parse(PayloadType::TXT_MSG, pkt.payload()).unwrap() else {
            panic!("datagramme");
        };
        let mut out = [0u8; 200];
        let opened = crypto::open(&s_ab, sealed, &mut out).unwrap();
        let t = TxtPlain::parse(opened).unwrap();
        prop_assert_eq!(t.text, text.as_bytes());
        prop_assert_eq!(t.ts, ts);
        prop_assert_eq!(t.ack_crc(a.pub_key()), ack);
    }

    /// Données d'annonce : encode puis decode redonne les champs (nom coupé en UTF-8 valide).
    #[test]
    fn app_data_roundtrip(
        kind in 0u8..16,
        loc in proptest::option::of(any::<(i32, i32)>()),
        name in "[a-zA-Z0-9 éàçù⛰-]{0,40}",
    ) {
        let ad = AppData { kind, location: loc, feat1: None, feat2: None, name: Some(name.as_bytes()) };
        let mut buf = [0u8; 32];
        let n = ad.encode(&mut buf).unwrap();
        let back = AppData::decode(&buf[..n]).unwrap();
        prop_assert_eq!(back.kind, kind);
        prop_assert_eq!(back.location, loc);
        prop_assert!(name.as_bytes().starts_with(back.name_str()));
        prop_assert!(core::str::from_utf8(back.name_str()).is_ok());
    }

    /// Données de canal : aller-retour par le constructeur et l'analyseur.
    #[test]
    fn group_data_roundtrip(dt in 1u16.., data in proptest::collection::vec(any::<u8>(), 0..166)) {
        let mut plain = [0u8; 200];
        let n = payload::group_data_plain(dt, &data, &mut plain).unwrap();
        let pkt = payload::group_datagram(PayloadType::GRP_DATA, &ChannelSecret::PUBLIC, &plain[..n]).unwrap();
        let Payload::Group { sealed, channel, .. } = Payload::parse(PayloadType::GRP_DATA, pkt.payload()).unwrap() else {
            panic!("canal");
        };
        prop_assert_eq!(channel, ChannelSecret::PUBLIC.hash());
        let mut out = [0u8; 200];
        let opened = crypto::open(&ChannelSecret::PUBLIC.0, sealed, &mut out).unwrap();
        let g = GroupDataPlain::parse(opened).unwrap();
        prop_assert_eq!(g.data_type, dt);
        prop_assert_eq!(g.data, &data[..]);
    }
}

#[test]
fn path_hops() {
    let mut b = PacketBuf::new(Route::Flood, PayloadType::TXT_MSG);
    for i in 0..63u8 {
        b.push_hop(&[i]).unwrap();
    }
    assert_eq!(b.path_len.hops(), 63);
    // Le C++ déborde ici (0x40) ; nous refusons le 64e saut.
    assert_eq!(b.push_hop(&[0xEE]), Err(mc_air::Error::PathTooLong));
    b.pop_front_hop().unwrap();
    assert_eq!(b.path()[0], 1);
    assert_eq!(b.path_len.hops(), 62);
    // Hashes de 3 octets : 21 sauts au plus (63 octets).
    let mut b = PacketBuf::new(Route::Flood, PayloadType::TXT_MSG);
    b.set_path(PathLen::new(3, 0).unwrap(), &[]).unwrap();
    for _ in 0..21 {
        b.push_hop(&[1, 2, 3]).unwrap();
    }
    assert_eq!(b.push_hop(&[1, 2, 3]), Err(mc_air::Error::PathTooLong));
}

#[test]
fn limits_match_cpp() {
    let a = LocalIdentity::from_seed(&[7; 32]);
    let s = [1u8; 32];
    // 160 octets de texte passent, 161 non ; 158 au plus avec un essai étendu.
    let mut p = [0u8; 200];
    assert!(payload::txt_plain(0, 0, 0, &[b'a'; 160], a.pub_key(), &mut p).is_ok());
    assert!(payload::txt_plain(0, 0, 0, &[b'a'; 161], a.pub_key(), &mut p).is_err());
    assert!(payload::txt_plain(0, 0, 4, &[b'a'; 159], a.pub_key(), &mut p).is_err());
    // Datagramme : clair + 2 + 15 ≤ 184.
    assert!(payload::datagram(PayloadType::TXT_MSG, 1, 2, &s, &[0; 167]).is_ok());
    assert!(payload::datagram(PayloadType::TXT_MSG, 1, 2, &s, &[0; 168]).is_err());
    // Données de canal : 165 au plus.
    assert!(payload::group_data_plain(1, &[0; 165], &mut p).is_ok());
    assert!(payload::group_data_plain(1, &[0; 166], &mut p).is_err());
    // Texte de canal : préfixe compris, 160 octets au plus.
    let n = payload::group_txt_plain(0, b"Alice", &[b'x'; 200], &mut p).unwrap();
    assert_eq!(n, 5 + 160);
}
