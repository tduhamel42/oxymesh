//! The MeshCore C++ companion's data (`examples/companion_radio/DataStore.cpp`, MeshCore, MIT;
//! `docs/spec/companion.md` §8.1), imported into oxymesh's storage.
//!
//! - `/identity/_main.id`: public key (32) then the expanded private key (64);
//! - `/prefs.json`: the settings (the older binary `/new_prefs` is not read: v1.18 rewrites it
//!   as `/prefs.json` at its first boot);
//! - `/contacts3`: 152-byte contact records, the same as oxymesh's;
//! - `/channels2`: 40 channel records of 68 bytes, index = channel number;
//! - `/bl/<first 8 key bytes in upper-case hex>`: a contact's last advert.

use mc_air::crypto::LocalIdentity;
use mc_node::contacts::{MAX_CHANNELS, MAX_CONTACTS};
use mc_node::device::Storage;
use mc_node::dispatch::RadioParams;
use mc_node::node::{Prefs, UiPrefs};
use mc_node::persist::{
    CHANNEL_RECORD, CONTACT_RECORD, decode_channel, decode_contact, encode_channel, encode_contact,
    encode_prefs,
};

use crate::json::{Value, fixed, unescape, walk};
use crate::{Error, File, Flash, Spiffs};

pub const IDENTITY: &str = "/identity/_main.id";
pub const PREFS: &str = "/prefs.json";
pub const CONTACTS: &str = "/contacts3";
pub const CHANNELS: &str = "/channels2";
/// Longest settings file read.
const PREFS_MAX: usize = 2048;
/// Longest stored advert.
const BLOB_MAX: usize = 255;

/// What an import brings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// The node's name (from the settings; zeros without them).
    pub name: [u8; 32],
    pub key: [u8; 32],
    pub contacts: u16,
    pub channels: u8,
    /// `None` when the settings file is missing or unreadable.
    pub radio: Option<RadioParams>,
    /// Stored adverts imported (0 in a summary).
    pub adverts: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportError {
    Spiffs(Error),
    /// No `/identity/_main.id`: nothing to import.
    NoIdentity,
    /// The private key does not give the stored public key.
    BadIdentity,
}

impl From<Error> for ImportError {
    fn from(e: Error) -> Self {
        Self::Spiffs(e)
    }
}

/// The C++ node's identity, checked.
pub fn identity<F: Flash>(fs: &mut Spiffs<F>) -> Result<LocalIdentity, ImportError> {
    let mut b = [0u8; 96];
    let data = match fs.read_all(IDENTITY, &mut b) {
        Ok(d) => d,
        Err(Error::NotFound) => return Err(ImportError::NoIdentity),
        Err(e) => return Err(e.into()),
    };
    let (Some(pub_key), Some(prv)) = (data.get(..32), data.get(32..96)) else {
        return Err(ImportError::BadIdentity);
    };
    let mut p = [0u8; 64];
    p.copy_from_slice(prv);
    let id = LocalIdentity::from_private(&p);
    if id.pub_key()[..] != *pub_key {
        return Err(ImportError::BadIdentity);
    }
    Ok(id)
}

/// The C++ settings over `base` (the fields oxymesh has); `None` without a readable file.
pub fn prefs<F: Flash>(fs: &mut Spiffs<F>, base: &Prefs) -> Option<Prefs> {
    let mut buf = [0u8; PREFS_MAX];
    let src = fs.read_all(PREFS, &mut buf).ok()?;
    let mut p = base.clone();
    let mut ok = true;
    let parsed = walk(src, |path, key, v| {
        let num = |d: u32| match v {
            Value::Bare(b) => fixed(b, d),
            Value::Str(_) => None,
        };
        let int = || num(0);
        let u8v = || int().and_then(|x| u8::try_from(x).ok());
        let section = path.first().copied().unwrap_or(b"");
        let mut set = || -> Option<()> {
            match (section, key) {
                (b"", b"name") => {
                    let Value::Str(s) = v else { return None };
                    p.name = [0; 32];
                    unescape(s, &mut p.name[..31]);
                }
                (b"", b"lat") => p.lat = i32::try_from(num(6)?).ok()?,
                (b"", b"lon") => p.lon = i32::try_from(num(6)?).ok()?,
                (b"radio", b"freq") => p.radio.freq_khz = u32::try_from(num(3)?).ok()?,
                (b"radio", b"bw") => p.radio.bw_hz = u32::try_from(num(3)?).ok()?,
                (b"radio", b"sf") => p.radio.sf = u8v()?,
                (b"radio", b"cr") => p.radio.cr = u8v()?,
                (b"radio", b"tx") => p.radio.tx_power_dbm = i8::try_from(int()?).ok()?,
                (b"radio", b"cad") => p.cad_enabled = int()? != 0,
                (b"radio", b"int_thr") => p.interference_threshold = u8v()?,
                (b"radio", b"rxgain") => p.rx_boosted_gain = int()? != 0,
                (b"radio", b"fem_rxgain") => p.fem_rxgain = int()? != 0,
                (b"radio", b"agc_int") => p.agc_reset_interval = u8v()?,
                (b"radio", b"af") => p.airtime_factor_milli = u32::try_from(num(3)?).ok()?,
                (b"radio", b"rxdelay") => p.rx_delay_base_milli = u32::try_from(num(3)?).ok()?,
                (b"radio", b"f_txdelay") => p.tx_delay_milli = u32::try_from(num(3)?).ok()?,
                (b"radio", b"d_txdelay") => {
                    p.direct_tx_delay_milli = u32::try_from(num(3)?).ok()?;
                }
                (b"radio", b"hash_mode") => p.path_hash_mode = u8v()?,
                (b"radio", b"multi_ack") => p.multi_acks = u8v()?,
                (b"gps", b"en") => p.gps_enabled = int()? != 0,
                (b"gps", b"int") => p.gps_interval = u32::try_from(int()?).ok()?.min(86_400),
                (b"gps", b"adv_loc") => p.advert_loc_policy = u8v()?,
                (b"repeat", b"disable") => p.repeat = int()? == 0,
                (b"comp", b"auto_max") => p.autoadd_max_hops = u8v()?,
                (b"comp", b"defs_nm") => {
                    let Value::Str(s) = v else { return None };
                    p.default_scope_name = [0; 31];
                    unescape(s, &mut p.default_scope_name[..30]);
                }
                (b"comp", b"defs_key") => {
                    let Value::Str(s) = v else { return None };
                    let mut k = [0u8; 16];
                    for (o, pair) in k.iter_mut().zip(s.as_chunks::<2>().0) {
                        let h = core::str::from_utf8(pair).ok()?;
                        *o = u8::from_str_radix(h, 16).ok()?;
                    }
                    p.default_scope_key = k;
                }
                (b"comp", b"pin") => p.ble_pin = u32::try_from(int()?).ok()?,
                (b"comp", b"buzz_q") => p.buzzer_quiet = int()? != 0,
                (b"comp", b"auto_add") => p.autoadd_config = u8v()?,
                (b"comp", b"man_add") => p.manual_add_contacts = u8v()?,
                (b"comp", b"tel_base") => p.telemetry_base = u8v()? & 3,
                (b"comp", b"tel_loc") => p.telemetry_loc = u8v()? & 3,
                (b"comp", b"tel_env") => p.telemetry_env = u8v()? & 3,
                // Hours in the C++ (`UITask.cpp:298`).
                (b"comp", b"tz_offset") => {
                    p.ui.tz_offset_min = i16::try_from(int()?).ok()?.clamp(-24, 28) * 60;
                }
                _ => {}
            }
            Some(())
        };
        if set().is_none() {
            ok = false;
        }
    });
    if !parsed || !ok {
        return None;
    }
    // The screen's buzzer follows the C++ mute.
    if p.buzzer_quiet {
        p.ui.buzzer = 0;
    }
    Some(p)
}

/// The contact records, checked (the C++ format is oxymesh's).
struct Contacts<'f, F> {
    fs: &'f mut Spiffs<F>,
    file: Option<File>,
    next: u32,
}

impl<F: Flash> Iterator for Contacts<'_, F> {
    type Item = [u8; CONTACT_RECORD];

    fn next(&mut self) -> Option<Self::Item> {
        let f = self.file?;
        loop {
            let at = self.next * CONTACT_RECORD as u32;
            if at + CONTACT_RECORD as u32 > f.size || self.next as usize >= MAX_CONTACTS {
                return None;
            }
            self.next += 1;
            let mut b = [0u8; CONTACT_RECORD];
            if self.fs.read_at(&f, at, &mut b).ok()? != CONTACT_RECORD {
                return None;
            }
            if let Some(c) = decode_contact(&b) {
                return Some(encode_contact(&c));
            }
        }
    }
}

fn open_opt<F: Flash>(fs: &mut Spiffs<F>, name: &str) -> Result<Option<File>, Error> {
    match fs.open(name) {
        Ok(f) => Ok(Some(f)),
        Err(Error::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The channel records by slot.
fn channels<F: Flash>(
    fs: &mut Spiffs<F>,
) -> Result<Option<[[u8; CHANNEL_RECORD]; MAX_CHANNELS]>, Error> {
    let Some(f) = open_opt(fs, CHANNELS)? else {
        return Ok(None);
    };
    let mut out = [[0u8; CHANNEL_RECORD]; MAX_CHANNELS];
    for (i, rec) in out.iter_mut().enumerate() {
        let at = (i * CHANNEL_RECORD) as u32;
        if at + CHANNEL_RECORD as u32 > f.size {
            break;
        }
        let mut b = [0u8; CHANNEL_RECORD];
        if fs.read_at(&f, at, &mut b)? == CHANNEL_RECORD
            && let Some(c) = decode_channel(&b)
        {
            *rec = encode_channel(&c);
        }
    }
    Ok(Some(out))
}

/// Channels in use: a name.
fn in_use(rec: &[u8; CHANNEL_RECORD]) -> bool {
    rec.get(4).is_some_and(|&c| c != 0)
}

/// What [`import`] would bring, without writing anything.
pub fn summary<F: Flash>(fs: &mut Spiffs<F>) -> Result<Summary, ImportError> {
    let id = identity(fs)?;
    let base = Prefs::defaults(id.pub_key());
    let p = prefs(fs, &base);
    let file = open_opt(fs, CONTACTS)?;
    let contacts = Contacts { fs, file, next: 0 }.count() as u16;
    let channels = channels(fs)?.map_or(0, |c| c.iter().filter(|r| in_use(r)).count() as u8);
    Ok(Summary {
        name: p.as_ref().map_or([0; 32], |p| p.name),
        key: *id.pub_key(),
        contacts,
        channels,
        radio: p.map(|p| p.radio),
        adverts: 0,
    })
}

/// Writes the C++ node into `st`: identity, settings (the screen's own kept from `ui`, except
/// what the C++ also has), contacts, the 40 channel slots, stored adverts. The runner reboots
/// afterwards to load them.
pub fn import<F: Flash>(
    fs: &mut Spiffs<F>,
    ui: UiPrefs,
    st: &mut impl Storage,
) -> Result<Summary, ImportError> {
    let id = identity(fs)?;
    let mut base = Prefs::defaults(id.pub_key());
    base.ui = UiPrefs {
        setup_done: true,
        ..ui
    };
    let p = prefs(fs, &base);
    // Everything read before anything is written: a bad file leaves the device as it was.
    let chans = channels(fs)?;
    let file = open_opt(fs, CONTACTS)?;

    st.save_identity(id.private_key(), id.pub_key());
    st.save_prefs(&encode_prefs(p.as_ref().unwrap_or(&base)));
    let mut n = 0u16;
    {
        let mut it = Contacts {
            fs: &mut *fs,
            file,
            next: 0,
        }
        .inspect(|_| n += 1);
        st.save_contacts(&mut it);
    }
    let mut channels = 0u8;
    if let Some(c) = &chans {
        for (i, rec) in c.iter().enumerate() {
            st.save_channel(i as u8, rec);
            channels += u8::from(in_use(rec));
        }
    }
    let adverts = adverts(fs, file, st)?;
    Ok(Summary {
        name: p.as_ref().map_or(base.name, |p| p.name),
        key: *id.pub_key(),
        contacts: n,
        channels,
        radio: p.map(|p| p.radio),
        adverts,
    })
}

/// `/bl/<hex>` files of imported contacts, stored under their full key.
fn adverts<F: Flash>(
    fs: &mut Spiffs<F>,
    contacts: Option<File>,
    st: &mut impl Storage,
) -> Result<u16, Error> {
    let Some(cf) = contacts else { return Ok(0) };
    // The blobs' names, found in one pass.
    let mut names: heapless::Vec<[u8; 8], MAX_CONTACTS> = heapless::Vec::new();
    fs.list(|name, _| {
        if let Some(hex) = name.strip_prefix(b"/bl/")
            && hex.len() == 16
        {
            let mut k = [0u8; 8];
            let ok = k.iter_mut().zip(hex.as_chunks::<2>().0).all(|(o, pair)| {
                core::str::from_utf8(pair)
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .map(|v| *o = v)
                    .is_some()
            });
            if ok {
                let _ = names.push(k);
            }
        }
    })?;
    let mut n = 0u16;
    let records = (cf.size as usize / CONTACT_RECORD).min(MAX_CONTACTS);
    for i in 0..records {
        let mut key = [0u8; 32];
        if fs.read_at(&cf, (i * CONTACT_RECORD) as u32, &mut key)? != 32 {
            break;
        }
        let Some(prefix) = key.get(..8) else { continue };
        if !names.iter().any(|k| k[..] == *prefix) {
            continue;
        }
        let mut name: heapless::String<20> = heapless::String::new();
        let _ = name.push_str("/bl/");
        for b in prefix {
            let _ = core::fmt::write(&mut name, format_args!("{b:02X}"));
        }
        let mut raw = [0u8; BLOB_MAX];
        if let Ok(data) = fs.read_all(&name, &mut raw) {
            st.put_blob(&key, data);
            n += 1;
        }
    }
    Ok(n)
}
