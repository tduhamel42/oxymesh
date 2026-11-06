//! Advert data (`app_data`, `docs/spec/air.md` §5; `AdvertDataHelpers.cpp:4-69`).

use mc_core::Writer;

use crate::Error;

/// At most 32 bytes of advert data (`MAX_ADVERT_DATA_SIZE`).
pub const APP_DATA_MAX: usize = 32;

pub const TYPE_NONE: u8 = 0;
pub const TYPE_CHAT: u8 = 1;
pub const TYPE_REPEATER: u8 = 2;
pub const TYPE_ROOM: u8 = 3;
pub const TYPE_SENSOR: u8 = 4;

const LATLON: u8 = 0x10;
const FEAT1: u8 = 0x20;
const FEAT2: u8 = 0x40;
const NAME: u8 = 0x80;

/// Decoded advert data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppData<'a> {
    /// Low nibble of the flags: 1 chat, 2 repeater, 3 room, 4 sensor.
    pub kind: u8,
    /// Latitude and longitude in degrees × 1e6.
    pub location: Option<(i32, i32)>,
    pub feat1: Option<u16>,
    pub feat2: Option<u16>,
    /// Raw name (UTF-8 expected, not checked); up to the first NUL when used as text.
    pub name: Option<&'a [u8]>,
}

impl<'a> AppData<'a> {
    /// Decodes like `AdvertDataParser`: requires at least the flags byte and the fields it
    /// announces; the name is the remainder if its flag is set, otherwise the remainder is ignored.
    pub fn decode(app_data: &'a [u8]) -> Result<Self, Error> {
        let mut r = mc_core::Reader::new(app_data);
        let flags = r.u8().map_err(|_| Error::Truncated)?;
        let location = if flags & LATLON != 0 {
            let lat = r.i32_le().map_err(|_| Error::Truncated)?;
            let lon = r.i32_le().map_err(|_| Error::Truncated)?;
            Some((lat, lon))
        } else {
            None
        };
        let feat1 = if flags & FEAT1 != 0 {
            Some(r.u16_le().map_err(|_| Error::Truncated)?)
        } else {
            None
        };
        let feat2 = if flags & FEAT2 != 0 {
            Some(r.u16_le().map_err(|_| Error::Truncated)?)
        } else {
            None
        };
        let rest = r.rest();
        let name = if flags & NAME != 0 && !rest.is_empty() {
            Some(rest)
        } else {
            None
        };
        Ok(Self {
            kind: flags & 0x0F,
            location,
            feat1,
            feat2,
            name,
        })
    }

    /// The name up to the first NUL (the C++ turns it into a C string).
    pub fn name_str(&self) -> &'a [u8] {
        let n = self.name.unwrap_or(&[]);
        let end = n.iter().position(|&b| b == 0).unwrap_or(n.len());
        n.get(..end).unwrap_or(&[])
    }

    /// Encodes like `AdvertDataBuilder::encodeTo`: the name is cut to the longest valid UTF-8
    /// prefix that fits in the 32 bytes; an empty name is not written.
    pub fn encode(&self, out: &mut [u8; APP_DATA_MAX]) -> Result<usize, Error> {
        let mut flags = self.kind & 0x0F;
        let mut w = Writer::new(out);
        w.put_u8(0).map_err(|_| Error::Overflow)?;
        if let Some((lat, lon)) = self.location {
            flags |= LATLON;
            w.put_i32_le(lat).map_err(|_| Error::Overflow)?;
            w.put_i32_le(lon).map_err(|_| Error::Overflow)?;
        }
        // The C++ writes feat1/feat2 only when they are non-zero.
        if let Some(f) = self.feat1.filter(|&f| f != 0) {
            flags |= FEAT1;
            w.put_u16_le(f).map_err(|_| Error::Overflow)?;
        }
        if let Some(f) = self.feat2.filter(|&f| f != 0) {
            flags |= FEAT2;
            w.put_u16_le(f).map_err(|_| Error::Overflow)?;
        }
        let name = self.name_str();
        let room = APP_DATA_MAX - w.position();
        let n = utf8_prefix_len(name, room);
        if n > 0 {
            flags |= NAME;
            w.put(name.get(..n).unwrap_or(&[]))
                .map_err(|_| Error::Overflow)?;
        }
        let len = w.position();
        w.patch_u8(0, flags).map_err(|_| Error::Overflow)?;
        Ok(len)
    }
}

/// Longest prefix of `s`, at most `max` bytes, made of complete, valid UTF-8 characters
/// (`validUtf8PrefixLength`, `UTF8Helpers.h:12`: same rules as `core::str::from_utf8`).
pub fn utf8_prefix_len(s: &[u8], max: usize) -> usize {
    let s = s.get(..s.len().min(max)).unwrap_or(&[]);
    match core::str::from_utf8(s) {
        Ok(_) => s.len(),
        Err(e) => e.valid_up_to(),
    }
}
