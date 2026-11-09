//! The device console (`docs/spec/companion.md` §6): any text an app or an allowed contact
//! sends to `RUN_CLI_COMMAND`, and the C helpers it parses numbers with.
//!
//! Oracle: never panic; the device answers with at most one CLI_REPLY frame that fits a
//! companion frame; the number parsers and the C float printer take any input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use mc_air::LocalIdentity;
use mc_node::device::{FrontEnd, Info, Storage};
use mc_node::history::HISTORY_RECORD;
use mc_node::persist::{CHANNEL_RECORD, CONTACT_RECORD, PREFS_RECORD};
use mc_node::{Device, Node, Prefs, cli};
use mc_proto::{FRAME_MAX, Response};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::SeedableRng;

struct Null;

impl Storage for Null {
    fn save_identity(&mut self, _: &[u8; 64], _: &[u8; 32]) {}
    fn save_prefs(&mut self, _: &[u8; PREFS_RECORD]) {}
    fn save_contacts(&mut self, records: &mut dyn Iterator<Item = [u8; CONTACT_RECORD]>) {
        records.for_each(drop);
    }
    fn save_channel(&mut self, _: u8, _: &[u8; CHANNEL_RECORD]) {}
    fn put_blob(&mut self, _: &[u8; 32], _: &[u8]) {}
    fn get_blob(&mut self, _: &[u8; 32], _: &mut [u8]) -> Option<usize> {
        None
    }
    fn remove_blob(&mut self, _: &[u8; 32]) {}
    fn usage_kb(&self) -> (u32, u32) {
        (0, 0)
    }
    fn put_queued(&mut self, _: u32, _: &[u8]) {}
    fn get_queued(&mut self, _: u32, _: &mut [u8]) -> Option<usize> {
        None
    }
    fn remove_queued(&mut self, _: u32) {}
    fn save_history(&mut self, _: u16, _: &[u8; HISTORY_RECORD]) {}
}

const INFO: Info = Info {
    manufacturer: "fuzz",
    version: "v1.18.0-oxymesh",
    build_date: "06-Nov-2026",
    max_tx_power: 22,
    has_gps: false,
    front_end: FrontEnd::PaLna,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = core::str::from_utf8(data) {
        let _ = cli::atoi(s);
        let _ = cli::strtod_micro(s);
        let mut p = Prefs::defaults(&[0; 32]);
        let mut out = cli::Reply::new();
        let _ = cli::radio_command(&mut p, s, &mut out);
    }
    if let Some(b) = data.get(..4) {
        let mut out = cli::Reply::new();
        cli::ftoa(&mut out, u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    }
    if let Some(b) = data.get(..10) {
        let num = i64::from_le_bytes(b[..8].try_into().unwrap());
        let den = u64::from(u16::from_le_bytes([b[8], b[9]]));
        let _ = cli::f32_bits(num, den);
    }

    let id = LocalIdentity::from_seed(&[7; 32]);
    let prefs = Prefs::defaults(id.pub_key());
    let now = mc_core::Instant::from_nanos(1_000_000_000);
    let mut dev = Device::new(Node::new(id, prefs, 1_793_900_000, now), INFO);
    let mut frame = vec![66u8];
    frame.extend_from_slice(data);
    let mut rng = ChaCha20Rng::seed_from_u64(1);
    dev.on_app_frame(now, &frame, &mut rng, &mut Null);
    let mut replies = 0;
    while let Some(f) = dev.poll_app() {
        assert!(f.len() <= FRAME_MAX);
        if matches!(Response::decode(&f), Ok(Response::CliReply { .. })) {
            replies += 1;
        }
    }
    assert!(replies <= 1);
});
