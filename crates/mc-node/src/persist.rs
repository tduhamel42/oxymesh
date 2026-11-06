//! Persistence (`docs/spec/companion.md` §8): binary records and write operations.
//!
//! Contacts and channels reuse the C++ firmware records (`/contacts3`, `/channels2`,
//! `DataStore.cpp:259-366`): the migration (MC6) reads them as is. Settings have their own
//! versioned format. The runner stores each record under a key; the contact order is restored
//! on load from their index.

use mc_core::{Reader, Writer};

use crate::contacts::{Channel, Contact};
use crate::dispatch::RadioParams;
use crate::node::Prefs;

pub const CONTACT_RECORD: usize = 152;
pub const CHANNEL_RECORD: usize = 68;
pub const PREFS_RECORD: usize = 128;
const PREFS_VERSION: u8 = 1;

/// Write requested from the runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOp {
    /// Identity: expanded private key (64) then public key (32), like `/identity/_main.id` reversed.
    Identity,
    Prefs,
    /// The whole contact table (write deferred by 5 s, like the C++).
    Contacts,
    /// One channel (index).
    Channel(u8),
}

/// `/contacts3`: pub 32, name 32, type, flags, unused, sync_since, out_path_len,
/// last_advert, out_path 64, lastmod, lat, lon.
pub fn encode_contact(c: &Contact) -> [u8; CONTACT_RECORD] {
    let mut out = [0u8; CONTACT_RECORD];
    let mut w = Writer::new(&mut out);
    let _ = (|| {
        w.put(&c.pub_key)?;
        w.put(&c.name)?;
        w.put_u8(c.kind)?;
        w.put_u8(c.flags)?;
        w.put_u8(0)?;
        w.put_u32_le(c.sync_since)?;
        w.put_u8(c.out_path_len)?;
        w.put_u32_le(c.last_advert)?;
        w.put(&c.out_path)?;
        w.put_u32_le(c.lastmod)?;
        w.put_i32_le(c.lat)?;
        w.put_i32_le(c.lon)
    })();
    out
}

pub fn decode_contact(b: &[u8]) -> Option<Contact> {
    let mut r = Reader::new(b);
    let key = *r.array::<32>().ok()?;
    let mut c = Contact::new(key);
    c.name = *r.array::<32>().ok()?;
    c.kind = r.u8().ok()?;
    c.flags = r.u8().ok()?;
    let _unused = r.u8().ok()?;
    c.sync_since = r.u32_le().ok()?;
    c.out_path_len = r.u8().ok()?;
    c.last_advert = r.u32_le().ok()?;
    c.out_path = *r.array::<64>().ok()?;
    c.lastmod = r.u32_le().ok()?;
    c.lat = r.i32_le().ok()?;
    c.lon = r.i32_le().ok()?;
    Some(c)
}

/// `/channels2`: 4 zero bytes, name 32, secret 32.
pub fn encode_channel(c: &Channel) -> [u8; CHANNEL_RECORD] {
    let mut out = [0u8; CHANNEL_RECORD];
    let mut w = Writer::new(&mut out);
    let _ = (|| {
        w.put(&[0; 4])?;
        w.put(&c.name)?;
        w.put(&c.secret.0)
    })();
    out
}

pub fn decode_channel(b: &[u8]) -> Option<Channel> {
    let mut r = Reader::new(b);
    let _ = r.array::<4>().ok()?;
    let name = *r.array::<32>().ok()?;
    let secret = *r.array::<32>().ok()?;
    Some(Channel {
        name,
        secret: mc_air::ChannelSecret(secret),
    })
}

/// Settings, version 1 (128 bytes).
pub fn encode_prefs(p: &Prefs) -> [u8; PREFS_RECORD] {
    let mut out = [0u8; PREFS_RECORD];
    let mut w = Writer::new(&mut out);
    let _ = (|| {
        w.put_u8(PREFS_VERSION)?;
        w.put(&p.name)?;
        w.put_i32_le(p.lat)?;
        w.put_i32_le(p.lon)?;
        w.put_u32_le(p.radio.freq_khz)?;
        w.put_u32_le(p.radio.bw_hz)?;
        w.put_u8(p.radio.sf)?;
        w.put_u8(p.radio.cr)?;
        w.put(&p.radio.tx_power_dbm.to_le_bytes())?;
        w.put_u32_le(p.airtime_factor_milli)?;
        w.put_u32_le(p.tx_delay_milli)?;
        w.put_u32_le(p.direct_tx_delay_milli)?;
        w.put_u32_le(p.rx_delay_base_milli)?;
        w.put_u8(p.multi_acks)?;
        w.put_u8(p.manual_add_contacts)?;
        w.put_u8(p.autoadd_config)?;
        w.put_u8(p.autoadd_max_hops)?;
        w.put_u8(p.telemetry_base | (p.telemetry_loc << 2) | (p.telemetry_env << 4))?;
        w.put_u8(p.advert_loc_policy)?;
        w.put_u32_le(p.ble_pin)?;
        let flags = u8::from(p.repeat)
            | (u8::from(p.cad_enabled) << 1)
            | (u8::from(p.gps_enabled) << 2)
            | (u8::from(p.buzzer_quiet) << 3);
        w.put_u8(flags)?;
        w.put_u8(p.path_hash_mode)?;
        w.put(&p.interference_threshold.to_le_bytes())?;
        w.put(&p.default_scope_name)?;
        w.put(&p.default_scope_key)
    })();
    out
}

/// Reads settings back; `None` if the version is unknown (factory settings are then kept).
pub fn decode_prefs(b: &[u8], defaults: &Prefs) -> Option<Prefs> {
    let mut r = Reader::new(b);
    if r.u8().ok()? != PREFS_VERSION {
        return None;
    }
    let mut p = defaults.clone();
    p.name = *r.array::<32>().ok()?;
    p.lat = r.i32_le().ok()?;
    p.lon = r.i32_le().ok()?;
    let freq_khz = r.u32_le().ok()?;
    let bw_hz = r.u32_le().ok()?;
    let sf = r.u8().ok()?;
    let cr = r.u8().ok()?;
    let tx_power_dbm = i8::from_le_bytes([r.u8().ok()?]);
    p.radio = RadioParams {
        freq_khz,
        bw_hz,
        sf,
        cr,
        tx_power_dbm,
    };
    p.airtime_factor_milli = r.u32_le().ok()?;
    p.tx_delay_milli = r.u32_le().ok()?;
    p.direct_tx_delay_milli = r.u32_le().ok()?;
    p.rx_delay_base_milli = r.u32_le().ok()?;
    p.multi_acks = r.u8().ok()?;
    p.manual_add_contacts = r.u8().ok()?;
    p.autoadd_config = r.u8().ok()?;
    p.autoadd_max_hops = r.u8().ok()?;
    let tel = r.u8().ok()?;
    p.telemetry_base = tel & 3;
    p.telemetry_loc = (tel >> 2) & 3;
    p.telemetry_env = (tel >> 4) & 3;
    p.advert_loc_policy = r.u8().ok()?;
    p.ble_pin = r.u32_le().ok()?;
    let flags = r.u8().ok()?;
    p.repeat = flags & 1 != 0;
    p.cad_enabled = flags & 2 != 0;
    p.gps_enabled = flags & 4 != 0;
    p.buzzer_quiet = flags & 8 != 0;
    p.path_hash_mode = r.u8().ok()?;
    p.interference_threshold = i8::from_le_bytes([r.u8().ok()?]);
    p.default_scope_name = *r.array::<31>().ok()?;
    p.default_scope_key = *r.array::<16>().ok()?;
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_roundtrip() {
        let mut c = Contact::new([7; 32]);
        c.set_name(b"Alice");
        c.kind = 1;
        c.flags = 0x11;
        c.out_path_len = 0x42;
        c.out_path[..4].copy_from_slice(&[1, 2, 3, 4]);
        c.last_advert = 10;
        c.lastmod = 20;
        c.lat = -5;
        c.lon = 6;
        c.sync_since = 99;
        let b = encode_contact(&c);
        let d = decode_contact(&b).unwrap();
        assert_eq!(encode_contact(&d), b);
        assert_eq!(&b[..32], &[7; 32]);
        assert_eq!(&b[32..37], b"Alice");
        assert_eq!(b[64], 1);

        let p = Prefs::defaults(&[0xAB; 32]);
        let mut q = p.clone();
        q.repeat = true;
        q.radio.cr = 8;
        q.default_scope_key = [3; 16];
        let back = decode_prefs(&encode_prefs(&q), &p).unwrap();
        assert_eq!(encode_prefs(&back), encode_prefs(&q));
        assert_eq!(&back.name[..8], b"ABABABAB");
    }
}
