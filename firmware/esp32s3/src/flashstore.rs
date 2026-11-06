//! Device storage in the `mcdata` partition (ADR-0006), using `sequential-storage`
//! (power-loss tolerant). Implements the `Storage` trait of `mc-node`.
//!
//! Keys (u32): 1 identity (96 B), 2 prefs, 3 contact count, `0x0001_0000 + i` contact i
//! (C++ `/contacts3` record), `0x0002_0000 + i` channel i, `0x8000_0000 | key prefix` stored
//! advert (preceded by the full key, checked on read).

use embassy_futures::block_on;
use embedded_storage_async::nor_flash::NorFlash;
use mc_node::Node;
use mc_node::persist::{
    CHANNEL_RECORD, CONTACT_RECORD, PREFS_RECORD, decode_channel, decode_contact, decode_prefs,
};
use sequential_storage::cache::{Cache, Uncached};
use sequential_storage::map::{MapConfig, MapStorage};

const KEY_IDENTITY: u32 = 1;
const KEY_PREFS: u32 = 2;
const KEY_COUNT: u32 = 3;
const CONTACT_BASE: u32 = 0x0001_0000;
const CHANNEL_BASE: u32 = 0x0002_0000;
const BLOB_BASE: u32 = 0x8000_0000;
const BUF: usize = 512;

/// CRC-32 (IEEE) of a record, so that only modified contacts are written.
fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn blob_key(key: &[u8; 32]) -> u32 {
    BLOB_BASE | (u32::from_le_bytes([key[0], key[1], key[2], key[3]]) >> 1)
}

pub struct FlashStore<S: NorFlash> {
    map: MapStorage<u32, S, Cache<Uncached, Uncached, Uncached, u32>>,
    buf: [u8; BUF],
    /// CRC of the written contacts, by index; 0 = absent.
    saved: [u32; mc_node::contacts::MAX_CONTACTS],
    saved_count: usize,
    len_kb: u32,
    pub errors: u32,
}

impl<S: NorFlash> FlashStore<S> {
    pub fn new(flash: S, len: u32) -> Option<Self> {
        let erase = S::ERASE_SIZE as u32;
        let config = MapConfig::try_new(0..(len - len % erase)).ok()?;
        Some(Self {
            map: MapStorage::new(flash, config, Cache::new_uncached()),
            buf: [0; BUF],
            saved: [0; mc_node::contacts::MAX_CONTACTS],
            saved_count: 0,
            len_kb: len / 1024,
            errors: 0,
        })
    }

    fn put(&mut self, key: u32, value: &[u8]) {
        if block_on(self.map.store_item(&mut self.buf, &key, &value)).is_err() {
            self.errors += 1;
        }
    }

    /// Deletes an entry by replacing it with an empty value (tombstone): `remove_item` would
    /// require a multiwrite flash. An empty entry is ignored on load.
    fn remove(&mut self, key: u32) {
        self.put(key, &[]);
    }

    fn get(&mut self, key: u32, out: &mut [u8]) -> Option<usize> {
        let v: Option<&[u8]> = block_on(self.map.fetch_item(&mut self.buf, &key)).ok()?;
        let v = v?;
        out.get_mut(..v.len())?.copy_from_slice(v);
        Some(v.len())
    }

    /// Stored identity: expanded private key (64) then public key (32).
    pub fn identity(&mut self) -> Option<[u8; 64]> {
        let mut b = [0u8; 96];
        (self.get(KEY_IDENTITY, &mut b)? == 96).then(|| {
            let mut prv = [0u8; 64];
            prv.copy_from_slice(&b[..64]);
            prv
        })
    }

    /// Reloads prefs, contacts (in index order) and channels into `node`.
    pub fn load(&mut self, node: &mut Node) {
        let mut b = [0u8; PREFS_RECORD];
        if self.get(KEY_PREFS, &mut b) == Some(PREFS_RECORD)
            && let Some(p) = decode_prefs(&b, &node.prefs)
        {
            node.prefs = p;
        }
        let mut cnt = [0u8; 2];
        let count = match self.get(KEY_COUNT, &mut cnt) {
            Some(2) => usize::from(u16::from_le_bytes(cnt)).min(mc_node::contacts::MAX_CONTACTS),
            _ => 0,
        };
        let mut rec = [0u8; CONTACT_RECORD];
        for i in 0..count {
            if self.get(CONTACT_BASE + i as u32, &mut rec) == Some(CONTACT_RECORD)
                && let Some(c) = decode_contact(&rec)
                && node.contacts.list.push(c).is_ok()
            {
                if let Some(s) = self.saved.get_mut(node.contacts.list.len() - 1) {
                    *s = crc32(&rec);
                }
            }
        }
        self.saved_count = node.contacts.list.len();
        let mut ch = [0u8; CHANNEL_RECORD];
        for i in 0..mc_node::contacts::MAX_CHANNELS {
            if self.get(CHANNEL_BASE + i as u32, &mut ch) == Some(CHANNEL_RECORD)
                && let Some(c) = decode_channel(&ch)
                && let Some(slot) = node.channels.0.get_mut(i)
            {
                *slot = c;
            }
        }
    }

    /// Erases the whole partition (`FACTORY_RESET`).
    pub fn erase_all(&mut self) {
        if block_on(self.map.erase_all()).is_err() {
            self.errors += 1;
        }
        self.saved = [0; mc_node::contacts::MAX_CONTACTS];
        self.saved_count = 0;
    }
}

impl<S: NorFlash> mc_node::Storage for FlashStore<S> {
    fn save_identity(&mut self, prv: &[u8; 64], pub_key: &[u8; 32]) {
        let mut b = [0u8; 96];
        b[..64].copy_from_slice(prv);
        b[64..].copy_from_slice(pub_key);
        self.put(KEY_IDENTITY, &b);
    }

    fn save_prefs(&mut self, rec: &[u8; PREFS_RECORD]) {
        self.put(KEY_PREFS, rec);
    }

    fn save_contacts(&mut self, records: &mut dyn Iterator<Item = [u8; CONTACT_RECORD]>) {
        let mut n = 0usize;
        for rec in records {
            let crc = crc32(&rec);
            if self.saved.get(n) != Some(&crc) {
                self.put(CONTACT_BASE + n as u32, &rec);
                if let Some(s) = self.saved.get_mut(n) {
                    *s = crc;
                }
            }
            n += 1;
        }
        for i in n..self.saved_count {
            self.remove(CONTACT_BASE + i as u32);
            if let Some(s) = self.saved.get_mut(i) {
                *s = 0;
            }
        }
        if n != self.saved_count {
            self.put(KEY_COUNT, &(n as u16).to_le_bytes());
            self.saved_count = n;
        }
    }

    fn save_channel(&mut self, idx: u8, rec: &[u8; CHANNEL_RECORD]) {
        self.put(CHANNEL_BASE + u32::from(idx), rec);
    }

    fn put_blob(&mut self, key: &[u8; 32], raw: &[u8]) {
        let mut b = [0u8; 32 + 255];
        let n = 32 + raw.len().min(255);
        b[..32].copy_from_slice(key);
        b[32..n].copy_from_slice(&raw[..n - 32]);
        self.put(blob_key(key), &b[..n]);
    }

    fn get_blob(&mut self, key: &[u8; 32], out: &mut [u8]) -> Option<usize> {
        let mut b = [0u8; 32 + 255];
        let n = self.get(blob_key(key), &mut b)?;
        if n < 32 || &b[..32] != key {
            return None;
        }
        let raw = &b[32..n];
        out.get_mut(..raw.len())?.copy_from_slice(raw);
        Some(raw.len())
    }

    fn remove_blob(&mut self, key: &[u8; 32]) {
        self.remove(blob_key(key));
    }

    fn usage_kb(&self) -> (u32, u32) {
        // Estimate: contacts and stored adverts.
        let used = (self.saved_count * (CONTACT_RECORD + 64 + 128)) as u32 / 1024;
        (used, self.len_kb)
    }
}
