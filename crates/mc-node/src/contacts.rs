//! Contacts et canaux (`ContactInfo.h`, `ChannelDetails.h`, `BaseChatMesh.cpp`).

use mc_air::crypto::{ChannelSecret, LocalIdentity, PUB_KEY_LEN, SECRET_LEN};
use mc_air::packet::PATH_MAX;

/// Contacts du build M5 (`MAX_CONTACTS`).
pub const MAX_CONTACTS: usize = 350;
/// Créneaux temporaires pour les requêtes anonymes (`MAX_ANON_CONTACTS`).
pub const ANON_SLOTS: usize = 8;
/// Canaux du build M5 (`MAX_GROUP_CHANNELS`).
pub const MAX_CHANNELS: usize = 40;
/// Contacts essayés pour un hash source (`MAX_SEARCH_RESULTS`).
pub const MAX_SEARCH: usize = 24;
/// « Chemin inconnu ».
pub const PATH_UNKNOWN: u8 = 0xFF;

pub const TYPE_NONE: u8 = 0;

#[derive(Clone)]
pub struct Contact {
    pub pub_key: [u8; PUB_KEY_LEN],
    /// Nom terminé par 0x00, complété de 0x00.
    pub name: [u8; 32],
    pub kind: u8,
    /// bit 0 favori, 1-3 télémétrie, 4 CLI à distance.
    pub flags: u8,
    pub out_path_len: u8,
    pub out_path: [u8; PATH_MAX],
    /// Horloge de l'émetteur.
    pub last_advert: u32,
    /// Notre horloge.
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

    /// Secret partagé, calculé une fois puis gardé (`getSharedSecret`).
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
        // `StrHelper::strncpy` : au plus 31 octets, puis 0x00.
        let name = name_str(name);
        for (o, b) in self.name.iter_mut().zip(name.iter().take(31)) {
            *o = *b;
        }
    }
}

/// La partie d'un nom avant le premier 0x00.
pub fn name_str(b: &[u8]) -> &[u8] {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b.get(..n).unwrap_or(&[])
}

/// Table des contacts, dans l'ordre du C++ (ajouts en fin, retraits qui décalent, écrasement sur
/// place), précédée des créneaux temporaires.
#[derive(Debug)]
pub struct Contacts {
    pub anon: [Option<Contact>; ANON_SLOTS],
    pub list: heapless::Vec<Contact, MAX_CONTACTS>,
}

impl Default for Contacts {
    fn default() -> Self {
        Self {
            anon: [const { None }; ANON_SLOTS],
            list: heapless::Vec::new(),
        }
    }
}

/// Où se trouve un contact.
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

    /// Recherche par clé complète, créneaux temporaires compris.
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

    /// Recherche par préfixe de clé (comparé sur `prefix.len()` octets), contacts ordinaires
    /// d'abord comme `lookupContactByPubKey`.
    pub fn find_prefix(&self, prefix: &[u8]) -> Option<Slot> {
        if let Some(i) = self.list.iter().position(|c| c.pub_key.starts_with(prefix)) {
            return Some(Slot::List(i));
        }
        self.anon
            .iter()
            .position(|c| c.as_ref().is_some_and(|c| c.pub_key.starts_with(prefix)))
            .map(Slot::Anon)
    }

    /// Contacts dont le hash d'1 octet vaut `h`, dans l'ordre du tableau C++ (temporaires
    /// d'abord), 24 au plus (`searchPeersByHash`).
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

    /// Le non-favori le plus ancien (`lastmod`), pour un écrasement quand la table est pleine.
    pub fn oldest_non_favourite(&self) -> Option<usize> {
        let mut best: Option<(usize, u32)> = None;
        for (i, c) in self.list.iter().enumerate() {
            if !c.is_favourite() && best.is_none_or(|(_, m)| c.lastmod < m) {
                best = Some((i, c.lastmod));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Un créneau temporaire libre, ou le plus ancien (`allocateContactSlot(true)`).
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

/// Un canal : nom (vide = créneau libre) et secret.
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

/// Les 40 canaux ; le canal 0 est préchargé « Public » (`MyMesh.cpp:1002-1003`).
#[derive(Debug, Clone)]
pub struct Channels(pub [Channel; MAX_CHANNELS]);

impl Default for Channels {
    fn default() -> Self {
        let mut c = [const { Channel::EMPTY }; MAX_CHANNELS];
        if let Some(first) = c.first_mut() {
            let mut name = [0u8; 32];
            for (o, b) in name.iter_mut().zip(b"Public") {
                *o = *b;
            }
            *first = Channel {
                name,
                secret: ChannelSecret::PUBLIC,
            };
        }
        Self(c)
    }
}

impl Channels {
    /// Canaux non vides dont le hash vaut `h`, 4 au plus (`searchChannelsByHash`).
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
