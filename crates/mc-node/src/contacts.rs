//! Contacts and channels (`ContactInfo.h`, `ChannelDetails.h`, `BaseChatMesh.cpp`).

use mc_air::crypto::{ChannelSecret, LocalIdentity, PUB_KEY_LEN, SECRET_LEN};
use mc_air::packet::PATH_MAX;

/// Contacts of the M5 build (`MAX_CONTACTS`).
pub const MAX_CONTACTS: usize = 350;
/// Temporary slots for anonymous requests (`MAX_ANON_CONTACTS`).
pub const ANON_SLOTS: usize = 8;
/// Channels of the M5 build (`MAX_GROUP_CHANNELS`).
pub const MAX_CHANNELS: usize = 40;
/// Contacts tried for a source hash (`MAX_SEARCH_RESULTS`).
pub const MAX_SEARCH: usize = 24;
/// "Path unknown".
pub const PATH_UNKNOWN: u8 = 0xFF;

pub const TYPE_NONE: u8 = 0;

#[derive(Clone)]
pub struct Contact {
    pub pub_key: [u8; PUB_KEY_LEN],
    /// 0x00-terminated name, padded with 0x00.
    pub name: [u8; 32],
    pub kind: u8,
    /// bit 0 favourite, 1-3 telemetry, 4 remote CLI.
    pub flags: u8,
    pub out_path_len: u8,
    pub out_path: [u8; PATH_MAX],
    /// Sender's clock.
    pub last_advert: u32,
    /// Our clock.
    pub lastmod: u32,
    pub lat: i32,
    pub lon: i32,
    pub sync_since: u32,
    secret: Option<[u8; SECRET_LEN]>,
}

impl core::fmt::Debug for Contact {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Contact({} {:?})",
            mc_core::Hex(&self.pub_key[..4]),
            crate::contacts::name_str(&self.name)
        )
    }
}

impl Contact {
    pub fn new(pub_key: [u8; PUB_KEY_LEN]) -> Self {
        Self {
            pub_key,
            name: [0; 32],
            kind: TYPE_NONE,
            flags: 0,
            out_path_len: PATH_UNKNOWN,
            out_path: [0; PATH_MAX],
            last_advert: 0,
            lastmod: 0,
            lat: 0,
            lon: 0,
            sync_since: 0,
            secret: None,
        }
    }

    pub const fn hash(&self) -> u8 {
        self.pub_key[0]
    }

    pub const fn is_favourite(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// Shared secret, computed once then kept (`getSharedSecret`).
    pub fn secret(&mut self, me: &LocalIdentity) -> Option<[u8; SECRET_LEN]> {
        if self.secret.is_none() {
            self.secret = me.shared_secret(&self.pub_key);
        }
        self.secret
    }

    pub fn forget_secret(&mut self) {
        self.secret = None;
    }

    pub fn out_path(&self) -> &[u8] {
        if self.out_path_len == PATH_UNKNOWN {
            return &[];
        }
        let n = mc_air::PathLen::from_byte(self.out_path_len)
            .map(|p| p.byte_len())
            .unwrap_or(0);
        self.out_path.get(..n).unwrap_or(&[])
    }

    pub fn set_name(&mut self, name: &[u8]) {
        self.name = [0; 32];
        // `StrHelper::strncpy`: at most 31 bytes, then 0x00.
        let name = name_str(name);
        for (o, b) in self.name.iter_mut().zip(name.iter().take(31)) {
            *o = *b;
        }
    }
}

/// The part of a name before the first 0x00.
pub fn name_str(b: &[u8]) -> &[u8] {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b.get(..n).unwrap_or(&[])
}

/// Contact table, in the C++ order (appends at the end, removals that shift, overwrites in
/// place), preceded by the temporary slots.
#[derive(Debug)]
pub struct Contacts {
    pub anon: [Option<Contact>; ANON_SLOTS],
    pub list: heapless::Vec<Contact, MAX_CONTACTS>,
}

impl Default for Contacts {
    fn default() -> Self {
        Self::new()
    }
}

impl Contacts {
    pub const fn new() -> Self {
        Self {
            anon: [const { None }; ANON_SLOTS],
            list: heapless::Vec::new(),
        }
    }
}

/// Where a contact lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Anon(usize),
    List(usize),
}

impl Contacts {
    pub fn get(&self, s: Slot) -> Option<&Contact> {
        match s {
            Slot::Anon(i) => self.anon.get(i).and_then(|c| c.as_ref()),
            Slot::List(i) => self.list.get(i),
        }
    }

    pub fn get_mut(&mut self, s: Slot) -> Option<&mut Contact> {
        match s {
            Slot::Anon(i) => self.anon.get_mut(i).and_then(|c| c.as_mut()),
            Slot::List(i) => self.list.get_mut(i),
        }
    }

    /// Lookup by full key, temporary slots included.
    pub fn find(&self, key: &[u8; PUB_KEY_LEN]) -> Option<Slot> {
        if let Some(i) = self
            .anon
            .iter()
            .position(|c| c.as_ref().is_some_and(|c| &c.pub_key == key))
        {
            return Some(Slot::Anon(i));
        }
        self.list
            .iter()
            .position(|c| &c.pub_key == key)
            .map(Slot::List)
    }

    /// Lookup by key prefix (compared over `prefix.len()` bytes), regular contacts first like
    /// `lookupContactByPubKey`.
    pub fn find_prefix(&self, prefix: &[u8]) -> Option<Slot> {
        if let Some(i) = self.list.iter().position(|c| c.pub_key.starts_with(prefix)) {
            return Some(Slot::List(i));
        }
        self.anon
            .iter()
            .position(|c| c.as_ref().is_some_and(|c| c.pub_key.starts_with(prefix)))
            .map(Slot::Anon)
    }

    /// Contacts whose 1-byte hash is `h`, in the C++ array order (temporary ones first), at most
    /// 24 (`searchPeersByHash`).
    pub fn by_hash(&self, h: u8) -> heapless::Vec<Slot, MAX_SEARCH> {
        let mut out = heapless::Vec::new();
        let anon = self
            .anon
            .iter()
            .enumerate()
            .filter(|(_, c)| c.as_ref().is_some_and(|c| c.hash() == h))
            .map(|(i, _)| Slot::Anon(i));
        let list = self
            .list
            .iter()
            .enumerate()
            .filter(|(_, c)| c.hash() == h)
            .map(|(i, _)| Slot::List(i));
        for s in anon.chain(list) {
            if out.push(s).is_err() {
                break;
            }
        }
        out
    }

    /// The oldest non-favourite (`lastmod`), to overwrite when the table is full.
    pub fn oldest_non_favourite(&self) -> Option<usize> {
        let mut best: Option<(usize, u32)> = None;
        for (i, c) in self.list.iter().enumerate() {
            if !c.is_favourite() && best.is_none_or(|(_, m)| c.lastmod < m) {
                best = Some((i, c.lastmod));
            }
        }
        best.map(|(i, _)| i)
    }

    /// A free temporary slot, or the oldest one (`allocateContactSlot(true)`).
    pub fn anon_slot(&mut self) -> usize {
        if let Some(i) = self.anon.iter().position(Option::is_none) {
            return i;
        }
        let mut best = (0usize, u32::MAX);
        for (i, c) in self.anon.iter().enumerate() {
            if let Some(c) = c
                && c.lastmod < best.1
            {
                best = (i, c.lastmod);
            }
        }
        best.0
    }
}

/// A channel: name (empty = free slot) and secret.
#[derive(Debug, Clone)]
pub struct Channel {
    pub name: [u8; 32],
    pub secret: ChannelSecret,
}

impl Channel {
    pub const EMPTY: Self = Self {
        name: [0; 32],
        secret: ChannelSecret([0; SECRET_LEN]),
    };

    pub fn is_empty(&self) -> bool {
        self.name.first().is_none_or(|&b| b == 0)
    }
}

/// The 40 channels; channel 0 is preloaded as "Public" (`MyMesh.cpp:1002-1003`).
#[derive(Debug, Clone)]
pub struct Channels(pub [Channel; MAX_CHANNELS]);

impl Default for Channels {
    fn default() -> Self {
        Self::new()
    }
}

impl Channels {
    pub const fn new() -> Self {
        let mut c = [const { Channel::EMPTY }; MAX_CHANNELS];
        c[0] = Channel {
            name: *b"Public\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
            secret: ChannelSecret::PUBLIC,
        };
        Self(c)
    }
}

impl Channels {
    /// Non-empty channels whose hash is `h`, at most 4 (`searchChannelsByHash`).
    pub fn by_hash(&self, h: u8) -> heapless::Vec<usize, 4> {
        let mut out = heapless::Vec::new();
        for (i, c) in self.0.iter().enumerate() {
            if !c.is_empty() && c.secret.hash() == h && out.push(i).is_err() {
                break;
            }
        }
        out
    }
}
