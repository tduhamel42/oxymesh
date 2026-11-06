//! Vectors from `testdata/air/vectors.toml`, produced by the independent Python oracle
//! (`testdata/air/gen.py`). Each packet is decoded, re-encoded identically, then rebuilt with the
//! crate's builders.

use mc_air::crypto::{self, ChannelSecret, LocalIdentity};
use mc_air::payload::{self, Advert, GroupDataPlain, GroupTxtPlain, PathPlain, TxtPlain};
use mc_air::{AppData, Packet, PacketBuf, PathLen, PayloadType, Route};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vectors {
    identity: Vec<Identity>,
    ecdh: Vec<Ecdh>,
    seal: Vec<Seal>,
    packet: Vec<Pkt>,
    advert: Vec<AdvertV>,
    txt: Vec<Txt>,
    group: Vec<Group>,
    path: Vec<PathV>,
    anon: Vec<Anon>,
    transport: Vec<Transport>,
    reject: Vec<Reject>,
}

#[derive(Deserialize)]
struct Identity {
    name: String,
    seed: String,
    prv: String,
    #[serde(rename = "pub")]
    pub_key: String,
}
#[derive(Deserialize)]
struct Ecdh {
    a: String,
    b: String,
    secret: String,
}
#[derive(Deserialize)]
struct Seal {
    secret: String,
    plain: String,
    sealed: String,
}
#[derive(Deserialize)]
struct Pkt {
    name: String,
    raw: String,
    route: u8,
    ptype: u8,
    path_len: u8,
    path: String,
    payload: String,
    transport: Option<[u16; 2]>,
    hash: String,
}
#[derive(Deserialize)]
struct AdvertV {
    packet: String,
    identity: String,
    ts: u32,
    app_data: String,
    kind: u8,
    lat: Option<i32>,
    lon: Option<i32>,
    adv_name: String,
}
#[derive(Deserialize)]
struct Txt {
    packet: String,
    from: String,
    to: String,
    ts: u32,
    attempt: u8,
    text: String,
    plain: String,
    ack: u32,
}
#[derive(Deserialize)]
struct Group {
    packet: String,
    secret: String,
    channel_hash: u8,
    plain: String,
    region: Option<String>,
    region_key: Option<String>,
}
#[derive(Deserialize)]
struct PathV {
    packet: String,
    in_path_len: u8,
    in_path: String,
    extra_type: u8,
    extra: String,
    plain: String,
}
#[derive(Deserialize)]
struct Anon {
    packet: String,
    from: String,
    to: String,
    secret: String,
    plain: String,
}
#[derive(Deserialize)]
struct Transport {
    region: String,
    ptype: u8,
    payload: String,
    code: u16,
}
#[derive(Deserialize)]
struct Reject {
    name: String,
    raw: String,
}

fn hx(s: &str) -> Vec<u8> {
    let mut out = vec![0u8; s.len() / 2];
    let n = mc_core::hex::decode_into(s, &mut out).expect("valid hexadecimal");
    out.truncate(n);
    out
}
fn arr<const N: usize>(s: &str) -> [u8; N] {
    hx(s).try_into().expect("length")
}

fn load() -> Vectors {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/air/vectors.toml"
    );
    toml::from_str(&std::fs::read_to_string(path).expect("vectors")).expect("valid TOML")
}

fn ident(v: &Vectors, name: &str) -> LocalIdentity {
    let i = v
        .identity
        .iter()
        .find(|i| i.name == name)
        .expect("identity");
    LocalIdentity::from_private(&arr(&i.prv))
}

fn pkt<'v>(v: &'v Vectors, name: &str) -> &'v Pkt {
    v.packet.iter().find(|p| p.name == name).expect("packet")
}

/// Copies the route, path and transport codes of a reference packet onto a built packet, then
/// compares the complete encoding.
fn assert_same_frame(mut built: PacketBuf, reference: &Pkt) {
    built.header.route = Route::from_bits(reference.route);
    built.transport = reference.transport;
    built
        .set_path(
            PathLen::from_byte(reference.path_len).unwrap(),
            &hx(&reference.path),
        )
        .unwrap();
    let mut out = [0u8; 255];
    let n = built.encode(&mut out).unwrap();
    assert_eq!(hex(&out[..n]), reference.raw, "{}", reference.name);
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn identities() {
    let v = load();
    for i in &v.identity {
        let from_seed = LocalIdentity::from_seed(&arr(&i.seed));
        assert_eq!(hex(from_seed.private_key()), i.prv, "{}", i.name);
        assert_eq!(hex(from_seed.pub_key()), i.pub_key, "{}", i.name);
        let from_prv = LocalIdentity::from_private(&arr(&i.prv));
        assert_eq!(hex(from_prv.pub_key()), i.pub_key, "{}", i.name);
        assert!(!from_prv.is_reserved());
    }
}

#[test]
fn ecdh_matches_orlp() {
    let v = load();
    for e in &v.ecdh {
        let a = ident(&v, &e.a);
        let b = ident(&v, &e.b);
        assert_eq!(hex(&a.shared_secret(b.pub_key()).unwrap()), e.secret);
        assert_eq!(hex(&b.shared_secret(a.pub_key()).unwrap()), e.secret);
    }
}

#[test]
fn seal_and_open() {
    let v = load();
    for s in &v.seal {
        let secret = arr(&s.secret);
        let plain = hx(&s.plain);
        let mut out = [0u8; 200];
        let n = crypto::seal(&secret, &plain, &mut out).unwrap();
        assert_eq!(hex(&out[..n]), s.sealed);
        let mut dec = [0u8; 200];
        let opened = crypto::open(&secret, &out[..n], &mut dec).unwrap();
        assert_eq!(&opened[..plain.len()], &plain[..]);
        assert!(opened[plain.len()..].iter().all(|&b| b == 0));
        // One bit flipped in the ciphertext: wrong MAC.
        let mut bad = out;
        bad[n - 1] ^= 1;
        assert_eq!(
            crypto::open(&secret, &bad[..n], &mut dec),
            Err(mc_air::Error::Mac)
        );
    }
}

#[test]
fn packets_roundtrip() {
    let v = load();
    for p in &v.packet {
        let raw = hx(&p.raw);
        let d = Packet::decode(&raw).unwrap_or_else(|e| panic!("{}: {e:?}", p.name));
        assert_eq!(d.header.route as u8, p.route, "{}", p.name);
        assert_eq!(d.header.ptype.value(), p.ptype, "{}", p.name);
        assert_eq!(d.path_len.byte(), p.path_len, "{}", p.name);
        assert_eq!(hex(d.path), p.path, "{}", p.name);
        assert_eq!(hex(d.payload), p.payload, "{}", p.name);
        assert_eq!(d.transport, p.transport, "{}", p.name);
        assert_eq!(hex(&d.hash()), p.hash, "{}", p.name);
        let mut out = [0u8; 255];
        let n = d.encode(&mut out).unwrap();
        assert_eq!(&out[..n], &raw[..], "{}", p.name);
        payload::Payload::parse(d.header.ptype, d.payload)
            .unwrap_or_else(|e| panic!("{}: {e:?}", p.name));
    }
}

#[test]
fn adverts() {
    let v = load();
    for a in &v.advert {
        let reference = pkt(&v, &a.packet);
        let raw = hx(&reference.raw);
        let d = Packet::decode(&raw).unwrap();
        let adv = Advert::parse(d.payload).unwrap();
        assert!(adv.verify(), "{}", a.packet);
        assert_eq!(adv.ts, a.ts);
        assert_eq!(hex(adv.app_data), a.app_data);
        let ad = AppData::decode(adv.app_data).unwrap();
        assert_eq!(ad.kind, a.kind);
        assert_eq!(ad.location, a.lat.zip(a.lon));
        assert_eq!(hex(ad.name_str()), a.adv_name);
        // Re-encode the advert data, then the complete advert (deterministic Ed25519).
        let mut buf = [0u8; 32];
        let n = ad.encode(&mut buf).unwrap();
        assert_eq!(hex(&buf[..n]), a.app_data, "{}", a.packet);
        let id = ident(&v, &a.identity);
        assert_same_frame(payload::advert(&id, a.ts, &buf[..n]).unwrap(), reference);
        // A tampered signature is rejected.
        let mut bad = raw.clone();
        bad[2 + reference.path.len() / 2 + 40] ^= 1;
        let d = Packet::decode(&bad).unwrap();
        assert!(!Advert::parse(d.payload).unwrap().verify());
    }
}

#[test]
fn direct_messages() {
    let v = load();
    for t in &v.txt {
        let reference = pkt(&v, &t.packet);
        let raw = hx(&reference.raw);
        let from = ident(&v, &t.from);
        let to = ident(&v, &t.to);
        let secret = to.shared_secret(from.pub_key()).unwrap();
        let d = Packet::decode(&raw).unwrap();
        let payload::Payload::Addressed {
            ptype,
            dest,
            src,
            sealed,
        } = payload::Payload::parse(d.header.ptype, d.payload).unwrap()
        else {
            panic!("{}: not an addressed datagram", t.packet);
        };
        assert_eq!(ptype, PayloadType::TXT_MSG);
        assert_eq!((dest, src), (to.hash(), from.hash()));
        let mut buf = [0u8; 200];
        let plain = crypto::open(&secret, sealed, &mut buf).unwrap();
        let tp = TxtPlain::parse(plain).unwrap();
        assert_eq!(tp.ts, t.ts);
        assert_eq!(hex(tp.text), t.text);
        assert_eq!(tp.attempt(), t.attempt & 3);
        assert_eq!(tp.txt_type(), 0);
        if t.attempt > 3 {
            assert_eq!(tp.after_nul, t.attempt);
        }
        assert_eq!(tp.ack_crc(from.pub_key()), t.ack);
        // Build on the sender side.
        let mut p = [0u8; 200];
        let (n, ack) =
            payload::txt_plain(t.ts, 0, t.attempt, &hx(&t.text), from.pub_key(), &mut p).unwrap();
        assert_eq!(hex(&p[..n]), t.plain);
        assert_eq!(ack, t.ack);
        let secret_a = from.shared_secret(to.pub_key()).unwrap();
        let built = payload::datagram(
            PayloadType::TXT_MSG,
            to.hash(),
            from.hash(),
            &secret_a,
            &p[..n],
        )
        .unwrap();
        assert_same_frame(built, reference);
    }
}

#[test]
fn channels() {
    let v = load();
    for g in &v.group {
        let reference = pkt(&v, &g.packet);
        let raw = hx(&reference.raw);
        let secret = ChannelSecret(arr(&g.secret));
        assert_eq!(secret.hash(), g.channel_hash);
        let d = Packet::decode(&raw).unwrap();
        let payload::Payload::Group {
            ptype,
            channel,
            sealed,
        } = payload::Payload::parse(d.header.ptype, d.payload).unwrap()
        else {
            panic!("{}: not a channel datagram", g.packet);
        };
        assert_eq!(channel, g.channel_hash);
        let mut buf = [0u8; 200];
        let plain = crypto::open(&secret.0, sealed, &mut buf).unwrap();
        let want = hx(&g.plain);
        assert_eq!(&plain[..want.len()], &want[..]);
        match ptype {
            PayloadType::GRP_TXT => {
                let gt = GroupTxtPlain::parse(plain).unwrap();
                assert_eq!(gt.text, &want[5..]);
            }
            PayloadType::GRP_DATA => {
                let gd = GroupDataPlain::parse(plain).unwrap();
                assert_eq!(gd.data_type, 0xFF4C);
                assert_eq!(gd.data, &want[3..]);
            }
            other => panic!("{other:?}"),
        }
        if let (Some(region), Some(key)) = (&g.region, &g.region_key) {
            assert_eq!(hex(&crypto::region_key(region)), *key);
            let code =
                crypto::transport_code(&crypto::region_key(region), ptype.value(), d.payload);
            assert_eq!(d.transport, Some([code, 0]));
        }
        let built = payload::group_datagram(ptype, &secret, &want).unwrap();
        assert_same_frame(built, reference);
    }
    // The Public channel and the "#test" channel derived from its name.
    assert_eq!(
        hex(&ChannelSecret::PUBLIC.0[..16]),
        "8b3387e9c5cdea6ac9e5edbaa115cd72"
    );
    let test = v
        .group
        .iter()
        .find(|g| g.packet == "grp_data_test")
        .unwrap();
    assert_eq!(hex(&ChannelSecret::from_hashtag("#test").0), test.secret);
    assert_eq!(
        ChannelSecret::from_hashtag("test"),
        ChannelSecret::from_hashtag("#test")
    );
}

#[test]
fn path_return() {
    let v = load();
    let alice = ident(&v, "alice");
    let bob = ident(&v, "bob");
    for p in &v.path {
        let reference = pkt(&v, &p.packet);
        let raw = hx(&reference.raw);
        let d = Packet::decode(&raw).unwrap();
        let payload::Payload::Addressed { sealed, .. } =
            payload::Payload::parse(d.header.ptype, d.payload).unwrap()
        else {
            panic!("PATH");
        };
        let secret = alice.shared_secret(bob.pub_key()).unwrap();
        let mut buf = [0u8; 200];
        let plain = crypto::open(&secret, sealed, &mut buf).unwrap();
        let pp = PathPlain::parse(plain).unwrap();
        assert_eq!(pp.path_len.byte(), p.in_path_len);
        assert_eq!(hex(pp.path), p.in_path);
        assert_eq!(pp.extra_type, p.extra_type);
        let extra = hx(&p.extra);
        assert_eq!(&pp.extra[..extra.len()], &extra[..]);
        let built = payload::path_return(
            alice.hash(),
            bob.hash(),
            &secret,
            pp.path_len,
            pp.path,
            Some((p.extra_type, &extra)),
            [0; 4],
        )
        .unwrap();
        assert_same_frame(built, reference);
        let _ = p.plain.len();
    }
}

#[test]
fn anonymous_requests() {
    let v = load();
    for a in &v.anon {
        let reference = pkt(&v, &a.packet);
        let from = ident(&v, &a.from);
        let to = ident(&v, &a.to);
        let secret = to.shared_secret(from.pub_key()).unwrap();
        assert_eq!(hex(&secret), a.secret);
        let built = payload::anon_datagram(to.hash(), &from, &secret, &hx(&a.plain)).unwrap();
        assert_same_frame(built, reference);
        let raw = hx(&reference.raw);
        let d = Packet::decode(&raw).unwrap();
        let payload::Payload::AnonReq { dest, sender, .. } =
            payload::Payload::parse(d.header.ptype, d.payload).unwrap()
        else {
            panic!("ANON_REQ");
        };
        assert_eq!(dest, to.hash());
        assert_eq!(sender, from.pub_key());
    }
}

#[test]
fn transport_codes() {
    let v = load();
    for t in &v.transport {
        assert_eq!(
            crypto::transport_code(&crypto::region_key(&t.region), t.ptype, &hx(&t.payload)),
            t.code,
            "{}",
            t.region
        );
    }
}

#[test]
fn rejects() {
    let v = load();
    for r in &v.reject {
        assert!(Packet::decode(&hx(&r.raw)).is_err(), "{} accepted", r.name);
    }
}
