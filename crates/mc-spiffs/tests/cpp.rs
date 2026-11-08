//! The C++ firmware's SPIFFS read back and imported (`testdata/spiffs/cpp.bin`, made by
//! `testdata/spiffs/gen.py`: made-up data, an independent writer).

use mc_air::crypto::ChannelSecret;
use mc_node::node::UiPrefs;
use mc_node::persist::{decode_channel, decode_contact, decode_prefs};
use mc_sim::MemStorage;
use mc_spiffs::cpp::{self, ImportError};
use mc_spiffs::{Error, Spiffs};

const IMAGE: &[u8] = include_bytes!("../../../testdata/spiffs/cpp.bin");
const NODE: &str = "99be776c87ee931219c1a9b844f0f5d72db95528f27dcc7118f51046ddccc231";
const ALICE: &str = "d92d16d3f66b42b703142ecc075974357dc6dcc66358fbb4d5c59eadd16ac1ff";

fn hex(s: &str) -> [u8; 32] {
    let mut k = [0u8; 32];
    for (i, o) in k.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
    }
    k
}

fn fs() -> Spiffs<&'static [u8]> {
    Spiffs::new(IMAGE, IMAGE.len() as u32).unwrap()
}

#[test]
fn lists_and_reads_files() {
    let mut fs = fs();
    let mut names = Vec::new();
    fs.list(|n, size| names.push((String::from_utf8_lossy(n).into_owned(), size)))
        .unwrap();
    names.sort();
    let alice_blob = format!("/bl/{}", ALICE[..16].to_uppercase());
    assert_eq!(
        names
            .iter()
            .map(|(n, s)| (n.as_str(), *s))
            .collect::<Vec<_>>(),
        vec![
            (alice_blob.as_str(), 100),
            ("/channels2", 40 * 68),
            ("/contacts3", 200 * 152),
            ("/identity/_main.id", 96),
            ("/prefs.json", 464),
        ],
        "the deleted older /prefs.json is not listed"
    );
    // A file over two index pages, read across page boundaries.
    let f = fs.open("/contacts3").unwrap();
    let mut all = vec![0u8; f.size as usize];
    assert_eq!(fs.read_at(&f, 0, &mut all).unwrap(), 30_400);
    let mut tail = [0u8; 152];
    assert_eq!(fs.read_at(&f, 199 * 152, &mut tail).unwrap(), 152);
    assert_eq!(&tail[..], &all[199 * 152..]);
    assert_eq!(&tail[32..40], b"Node 199");
    assert_eq!(
        fs.read_at(&f, 30_390, &mut tail).unwrap(),
        10,
        "short read at the end"
    );
    assert_eq!(fs.open("/nope"), Err(Error::NotFound));
}

#[test]
fn identity_and_settings() {
    let mut fs = fs();
    let id = cpp::identity(&mut fs).unwrap();
    assert_eq!(*id.pub_key(), hex(NODE));
    let base = mc_node::Prefs::defaults(id.pub_key());
    let p = cpp::prefs(&mut fs, &base).unwrap();
    assert_eq!(&p.name[..14], b"C++ M5 \"test\"\0");
    assert_eq!((p.lat, p.lon), (45_123_456, -5_654_321));
    assert_eq!(
        (p.radio.freq_khz, p.radio.bw_hz, p.radio.sf, p.radio.cr),
        (869_432, 125_000, 9, 6)
    );
    assert_eq!(p.radio.tx_power_dbm, 20);
    assert_eq!(
        (
            p.airtime_factor_milli,
            p.tx_delay_milli,
            p.direct_tx_delay_milli,
            p.rx_delay_base_milli
        ),
        (1500, 500, 200, 0)
    );
    assert_eq!((p.path_hash_mode, p.multi_acks), (1, 1));
    assert_eq!(
        (p.gps_enabled, p.gps_interval, p.advert_loc_policy),
        (true, 60, 1)
    );
    assert!(!p.repeat);
    assert_eq!(
        (p.autoadd_max_hops, p.autoadd_config, p.manual_add_contacts),
        (3, 2, 1)
    );
    assert_eq!(p.default_scope_key, core::array::from_fn(|i| i as u8));
    assert_eq!(p.ble_pin, 123_456);
    assert!(p.buzzer_quiet);
    assert_eq!(p.ui.buzzer, 0, "the screen's buzzer follows the C++ mute");
    assert_eq!(
        (p.telemetry_base, p.telemetry_loc, p.telemetry_env),
        (2, 1, 0)
    );
    assert_eq!(p.ui.tz_offset_min, 60);
}

#[test]
fn summary_then_import() {
    let mut fs = fs();
    let s = cpp::summary(&mut fs).unwrap();
    assert_eq!(s.key, hex(NODE));
    assert_eq!((s.contacts, s.channels), (200, 2));
    assert_eq!(s.radio.unwrap().freq_khz, 869_432);
    assert_eq!(&s.name[..6], b"C++ M5");

    let mut st = MemStorage::default();
    // The screen's own settings stay, except what the C++ also has.
    let ui = UiPrefs {
        compact: true,
        muted: 5,
        ..UiPrefs::DEFAULT
    };
    let r = cpp::import(&mut fs, ui, &mut st).unwrap();
    assert_eq!((r.contacts, r.channels, r.adverts), (200, 2, 1));
    let (prv, pub_key) = st.identity.unwrap();
    assert_eq!(pub_key, hex(NODE));
    assert_eq!(
        mc_air::crypto::LocalIdentity::from_private(&prv).pub_key(),
        &pub_key
    );
    let p = decode_prefs(
        st.prefs.as_ref().unwrap(),
        &mc_node::Prefs::defaults(&pub_key),
    )
    .unwrap();
    assert_eq!(p.radio.freq_khz, 869_432);
    assert!(p.ui.compact && p.ui.setup_done);
    assert_eq!((p.ui.muted, p.ui.buzzer, p.ui.tz_offset_min), (5, 0, 60));
    assert_eq!(st.contacts.len(), 200);
    let alice = decode_contact(&st.contacts[0]).unwrap();
    assert_eq!(alice.pub_key, hex(ALICE));
    assert_eq!(&alice.name[..5], b"Alice");
    assert_eq!((alice.lat, alice.lon), (45_000_000, 6_000_000));
    assert_eq!(
        st.channels.len(),
        40,
        "every slot written, free ones emptied"
    );
    let public = decode_channel(&st.channels[&0]).unwrap();
    assert_eq!(public.secret, ChannelSecret::PUBLIC);
    let test = decode_channel(&st.channels[&1]).unwrap();
    assert_eq!(test.secret, ChannelSecret::from_hashtag("#test"));
    assert!(decode_channel(&st.channels[&2]).unwrap().is_empty());
    assert_eq!(st.blobs[&hex(ALICE)], (0..100).collect::<Vec<u8>>());
}

#[test]
fn what_is_refused() {
    assert_eq!(
        Spiffs::new(&[0xFFu8; 8192][..], 8192).err(),
        Some(Error::NotSpiffs)
    );
    // The identity's private key changed: refused, nothing written.
    let mut img = IMAGE.to_vec();
    let mut fs = Spiffs::new(&img[..], img.len() as u32).unwrap();
    let f = fs.open("/identity/_main.id").unwrap();
    let mut buf = [0u8; 96];
    fs.read_at(&f, 0, &mut buf).unwrap();
    let at = img
        .windows(96)
        .position(|w| w == buf)
        .expect("the identity in one data page");
    img[at + 40] ^= 1;
    let mut fs = Spiffs::new(&img[..], img.len() as u32).unwrap();
    let mut st = MemStorage::default();
    assert_eq!(
        cpp::import(&mut fs, UiPrefs::DEFAULT, &mut st).err(),
        Some(ImportError::BadIdentity)
    );
    assert!(st.identity.is_none() && st.prefs.is_none());
}
