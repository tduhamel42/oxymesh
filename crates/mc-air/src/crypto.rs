//! MeshCore crypto (`docs/spec/air.md` §2): AES-128-ECB, 2-byte HMAC-SHA256 MAC placed in front,
//! orlp identities with a 64-byte expanded private key, orlp-style ECDH, hashes.
//!
//! These primitives are weak (ECB, 2-byte MAC, static keys): we reproduce them for compatibility
//! (ADR-0001), not because they are good.

use aes::Aes128;
use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use curve25519_dalek::montgomery::MontgomeryPoint;
use curve25519_dalek::scalar::Scalar;
use ed25519_dalek::hazmat::{ExpandedSecretKey, raw_sign_byupdate};
use ed25519_dalek::{Signature, VerifyingKey};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};

use crate::Error;

pub const PUB_KEY_LEN: usize = 32;
pub const PRV_KEY_LEN: usize = 64;
pub const SIG_LEN: usize = 64;
pub const SECRET_LEN: usize = 32;
pub const MAC_LEN: usize = 2;
pub const BLOCK_LEN: usize = 16;

type HmacSha256 = Hmac<Sha256>;

/// SHA-256 of several parts, truncated to `N` bytes.
pub fn sha256_trunc<const N: usize>(parts: &[&[u8]]) -> [u8; N] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    let full = h.finalize();
    let mut out = [0u8; N];
    for (o, b) in out.iter_mut().zip(full.iter()) {
        *o = *b;
    }
    out
}

fn hmac_sha256(key: &[u8], data: &[&[u8]]) -> [u8; 32] {
    // HMAC accepts a key of any length: `new_from_slice` cannot fail.
    let mut m = match <HmacSha256 as KeyInit>::new_from_slice(key) {
        Ok(m) => m,
        Err(_) => return [0; 32],
    };
    for d in data {
        m.update(d);
    }
    let full = m.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&full);
    out
}

/// Length of the sealed block (MAC + AES) for `plain_len` bytes of plaintext.
pub const fn sealed_len(plain_len: usize) -> usize {
    MAC_LEN + plain_len.div_ceil(BLOCK_LEN) * BLOCK_LEN
}

/// `MAC(2) ‖ AES-128-ECB(zero-padded plaintext)` (`Utils.cpp:85-145`).
/// AES key = `secret[0..16]`, HMAC key = `secret[0..32]`. Returns the length written.
pub fn seal(secret: &[u8; SECRET_LEN], plain: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    let n = sealed_len(plain.len());
    let out = out.get_mut(..n).ok_or(Error::Overflow)?;
    let (mac, body) = out.split_at_mut(MAC_LEN);
    let aes = aes_key(secret);
    for (i, block) in body.as_chunks_mut::<BLOCK_LEN>().0.iter_mut().enumerate() {
        *block = [0u8; BLOCK_LEN];
        let start = i * BLOCK_LEN;
        let src = plain
            .get(start..plain.len().min(start + BLOCK_LEN))
            .unwrap_or(&[]);
        if let Some(dst) = block.get_mut(..src.len()) {
            dst.copy_from_slice(src);
        }
        aes.encrypt_block(block.into());
    }
    let tag = hmac_sha256(secret, &[body]);
    mac.copy_from_slice(tag.get(..MAC_LEN).unwrap_or(&[0; MAC_LEN]));
    Ok(n)
}

/// Checks the MAC, then decrypts (`Utils.cpp:147-175`). The plaintext keeps the zero padding of
/// the last block, as in C++. Rejects a ciphertext that is empty or not a multiple of 16, or a
/// wrong MAC.
pub fn open<'o>(
    secret: &[u8; SECRET_LEN],
    sealed: &[u8],
    out: &'o mut [u8],
) -> Result<&'o [u8], Error> {
    let (mac, body) = sealed.split_at_checked(MAC_LEN).ok_or(Error::Truncated)?;
    if body.is_empty() || body.len() % BLOCK_LEN != 0 {
        return Err(Error::CipherLen);
    }
    let tag = hmac_sha256(secret, &[body]);
    // Constant-time comparison (2 bytes: on principle).
    let diff = mac
        .iter()
        .zip(tag.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if diff != 0 {
        return Err(Error::Mac);
    }
    let out = out.get_mut(..body.len()).ok_or(Error::Overflow)?;
    let aes = aes_key(secret);
    for (src, dst) in body
        .as_chunks::<BLOCK_LEN>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<BLOCK_LEN>().0)
    {
        *dst = *src;
        aes.decrypt_block(dst.into());
    }
    Ok(out)
}

fn aes_key(secret: &[u8; SECRET_LEN]) -> Aes128 {
    let mut k = [0u8; 16];
    for (o, b) in k.iter_mut().zip(secret.iter()) {
        *o = *b;
    }
    Aes128::new(&k.into())
}

/// Verifies an Ed25519 signature (non-strict verification, like rweather's `Ed25519::verify`).
pub fn verify(pub_key: &[u8; PUB_KEY_LEN], message: &[&[u8]], sig: &[u8; SIG_LEN]) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(pub_key) else {
        return false;
    };
    let sig = Signature::from_bytes(sig);
    let mut buf = [0u8; 32 + 4 + 32];
    let mut n = 0usize;
    for part in message {
        let Some(dst) = buf.get_mut(n..n + part.len()) else {
            return false;
        };
        dst.copy_from_slice(part);
        n += part.len();
    }
    let msg = buf.get(..n).unwrap_or(&[]);
    ed25519_dalek::Verifier::verify(&vk, msg, &sig).is_ok()
}

/// Local identity in MeshCore's format: 64-byte "expanded" orlp private key
/// (`scalar(32) ‖ nonce prefix(32)`, the seed is not kept) and public key.
#[derive(Clone)]
pub struct LocalIdentity {
    prv: [u8; PRV_KEY_LEN],
    pub_key: [u8; PUB_KEY_LEN],
}

impl core::fmt::Debug for LocalIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "LocalIdentity({})", mc_core::Hex(&self.pub_key))
    }
}

impl LocalIdentity {
    /// Empty identity (all-zero keys), only to reserve a static slot before replacing it with a
    /// real identity. It neither signs nor decrypts anything useful.
    pub const PLACEHOLDER: Self = Self {
        prv: [0; PRV_KEY_LEN],
        pub_key: [0; PUB_KEY_LEN],
    };

    /// Identity derived from a 32-byte seed, like orlp's `ed25519_create_keypair`:
    /// `SHA512(seed)`, clamped scalar.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let h = Sha512::digest(seed);
        let mut prv = [0u8; PRV_KEY_LEN];
        prv.copy_from_slice(&h);
        if let Some(b) = prv.first_mut() {
            *b &= 248;
        }
        if let Some(b) = prv.get_mut(31) {
            *b &= 63;
            *b |= 64;
        }
        Self::from_private(&prv)
    }

    /// Identity from an expanded private key (`_main.id` file, `IMPORT_PRIVATE_KEY`).
    /// As in orlp, the scalar is taken as is (reduced modulo the order), without clamping.
    pub fn from_private(prv: &[u8; PRV_KEY_LEN]) -> Self {
        let esk = expanded(prv);
        let pub_key = VerifyingKey::from(&esk).to_bytes();
        Self { prv: *prv, pub_key }
    }

    pub const fn pub_key(&self) -> &[u8; PUB_KEY_LEN] {
        &self.pub_key
    }

    pub const fn private_key(&self) -> &[u8; PRV_KEY_LEN] {
        &self.prv
    }

    /// 1-byte hash of the identity (`pub[0]`).
    pub const fn hash(&self) -> u8 {
        self.pub_key[0]
    }

    /// MeshCore rejects public keys starting with 0x00 or 0xFF (`Identity.cpp:71-72`).
    pub const fn is_reserved(&self) -> bool {
        self.pub_key[0] == 0x00 || self.pub_key[0] == 0xFF
    }

    /// Ed25519 signature of the concatenated parts (`sign.c`), without copying the message.
    pub fn sign(&self, message: &[&[u8]]) -> Result<[u8; SIG_LEN], Error> {
        let esk = expanded(&self.prv);
        let vk = VerifyingKey::from(&esk);
        let sig = raw_sign_byupdate::<Sha512, _>(
            &esk,
            |h: &mut Sha512| {
                for part in message {
                    h.update(part);
                }
                Ok(())
            },
            &vk,
        )
        .map_err(|_| Error::Signature)?;
        Ok(sig.to_bytes())
    }

    /// Secret shared with a peer (`key_exchange.c`): X25519 between the clamped scalar and the
    /// Montgomery form `u = (1+y)/(1-y)` of the peer's Ed25519 key. `None` if the peer's key does
    /// not decompress (such a key cannot sign an advert: no real peer has one).
    pub fn shared_secret(&self, peer: &[u8; PUB_KEY_LEN]) -> Option<[u8; SECRET_LEN]> {
        let vk = VerifyingKey::from_bytes(peer).ok()?;
        let u: MontgomeryPoint = vk.to_montgomery();
        let mut e = [0u8; 32];
        e.copy_from_slice(self.prv.get(..32)?);
        Some(u.mul_clamped(e).to_bytes())
    }
}

fn expanded(prv: &[u8; PRV_KEY_LEN]) -> ExpandedSecretKey {
    let mut scalar = [0u8; 32];
    let mut hash_prefix = [0u8; 32];
    let (a, b) = prv.split_at(32);
    scalar.copy_from_slice(a);
    hash_prefix.copy_from_slice(b);
    ExpandedSecretKey {
        scalar: Scalar::from_bytes_mod_order(scalar),
        hash_prefix,
    }
}

/// Channel secret: 16- or 32-byte PSK, zero-padded to 32.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChannelSecret(pub [u8; SECRET_LEN]);

impl core::fmt::Debug for ChannelSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ChannelSecret(#{:02x})", self.hash())
    }
}

impl ChannelSecret {
    /// Preloaded "Public" channel (`8b3387e9c5cdea6ac9e5edbaa115cd72`, comp:114).
    pub const PUBLIC: Self = Self::from_psk16(&[
        0x8b, 0x33, 0x87, 0xe9, 0xc5, 0xcd, 0xea, 0x6a, 0xc9, 0xe5, 0xed, 0xba, 0xa1, 0x15, 0xcd,
        0x72,
    ]);

    pub const fn from_psk16(psk: &[u8; 16]) -> Self {
        let mut s = [0u8; SECRET_LEN];
        let (lo, _) = s.split_at_mut(16);
        lo.copy_from_slice(psk);
        Self(s)
    }

    /// "#name" channel: `SHA256("#name")[0..16]` (derived by the app, `companion_protocol.md:462-466`).
    pub fn from_hashtag(name: &str) -> Self {
        let name = name.strip_prefix('#').unwrap_or(name);
        let k: [u8; 16] = sha256_trunc(&[b"#", name.as_bytes()]);
        Self::from_psk16(&k)
    }

    /// 1-byte channel hash: `SHA256(secret[0..16])[0]` if the last 16 bytes are zero,
    /// otherwise `SHA256(secret[0..32])[0]` (`BaseChatMesh.cpp:933-946`).
    pub fn hash(&self) -> u8 {
        let (lo, hi) = self.0.split_at(16);
        let key = if hi.iter().all(|&b| b == 0) {
            lo
        } else {
            &self.0[..]
        };
        let [h] = sha256_trunc::<1>(&[key]);
        h
    }
}

/// Ack CRC of a text message: `SHA256(ts ‖ flags ‖ text ‖ sender pub)[0..4]`,
/// read as a little-endian u32 (`BaseChatMesh.cpp:245, 462`).
pub fn ack_crc(ts: u32, flags: u8, text: &[u8], sender_pub: &[u8; PUB_KEY_LEN]) -> u32 {
    let h: [u8; 4] = sha256_trunc(&[&ts.to_le_bytes(), &[flags], text, sender_pub]);
    u32::from_le_bytes(h)
}

/// Region key: `SHA256("#" ‖ name)[0..16]`; "fr" and "#fr" give the same key
/// (`RegionMap.cpp:173-188`).
pub fn region_key(name: &str) -> [u8; 16] {
    let name = name.strip_prefix('#').unwrap_or(name);
    sha256_trunc(&[b"#", name.as_bytes()])
}

/// Transport (scope) code: `HMAC-SHA256(key16, type ‖ payload)[0..2]` as a little-endian u16;
/// 0 becomes 1 and 0xFFFF becomes 0xFFFE (`TransportKeyStore.cpp:4-18`).
pub fn transport_code(key: &[u8; 16], ptype: u8, payload: &[u8]) -> u16 {
    let h = hmac_sha256(key, &[&[ptype], payload]);
    let [a, b, ..] = h;
    match u16::from_le_bytes([a, b]) {
        0 => 1,
        0xFFFF => 0xFFFE,
        c => c,
    }
}
