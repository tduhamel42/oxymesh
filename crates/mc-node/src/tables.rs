//! Anti-doublon (`SimpleMeshTables.h`) : anneau de 160 hashes de 8 octets, vérifié avant tout
//! déchiffrement ; nos propres émissions y sont inscrites.

use mc_air::packet::PACKET_HASH_LEN;

pub const SEEN_MAX: usize = 128 + 32;

#[derive(Debug, Clone)]
pub struct Seen {
    hashes: [[u8; PACKET_HASH_LEN]; SEEN_MAX],
    next: usize,
    pub direct_dups: u32,
    pub flood_dups: u32,
}

impl Default for Seen {
    fn default() -> Self {
        Self {
            hashes: [[0; PACKET_HASH_LEN]; SEEN_MAX],
            next: 0,
            direct_dups: 0,
            flood_dups: 0,
        }
    }
}

impl Seen {
    pub fn was_seen(&mut self, hash: &[u8; PACKET_HASH_LEN], direct: bool) -> bool {
        let hit = self.hashes.iter().any(|h| h == hash);
        if hit {
            if direct {
                self.direct_dups = self.direct_dups.wrapping_add(1);
            } else {
                self.flood_dups = self.flood_dups.wrapping_add(1);
            }
        }
        hit
    }

    pub fn mark(&mut self, hash: &[u8; PACKET_HASH_LEN]) {
        if let Some(slot) = self.hashes.get_mut(self.next) {
            *slot = *hash;
        }
        self.next = (self.next + 1) % SEEN_MAX;
    }

    /// Vérifie et inscrit d'un coup ; vrai si la trame est nouvelle.
    pub fn check_and_mark(&mut self, hash: &[u8; PACKET_HASH_LEN], direct: bool) -> bool {
        if self.was_seen(hash, direct) {
            return false;
        }
        self.mark(hash);
        true
    }
}
