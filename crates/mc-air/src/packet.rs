//! The packet: header, transport codes, path, payload (`docs/spec/air.md` §1).

use mc_core::{Overflow, Writer};
use sha2::{Digest, Sha256};

use crate::Error;

/// Longest payload (`MAX_PACKET_PAYLOAD`).
pub const PAYLOAD_MAX: usize = 184;
/// Longest path, in bytes (`MAX_PATH_SIZE`).
pub const PATH_MAX: usize = 64;
/// Duplicate-detection hash, in bytes (`MAX_HASH_SIZE`).
pub const PACKET_HASH_LEN: usize = 8;

/// Route type: header bits 0-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    TransportFlood = 0,
    Flood = 1,
    Direct = 2,
    TransportDirect = 3,
}

impl Route {
    pub const fn from_bits(b: u8) -> Self {
        match b & 3 {
            0 => Self::TransportFlood,
            1 => Self::Flood,
            2 => Self::Direct,
            _ => Self::TransportDirect,
        }
    }
    pub const fn is_flood(self) -> bool {
        matches!(self, Self::TransportFlood | Self::Flood)
    }
    pub const fn is_direct(self) -> bool {
        !self.is_flood()
    }
    pub const fn has_transport_codes(self) -> bool {
        matches!(self, Self::TransportFlood | Self::TransportDirect)
    }
}

/// Payload type: header bits 2-5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PayloadType(u8);

impl PayloadType {
    pub const REQ: Self = Self(0x0);
    pub const RESPONSE: Self = Self(0x1);
    pub const TXT_MSG: Self = Self(0x2);
    pub const ACK: Self = Self(0x3);
    pub const ADVERT: Self = Self(0x4);
    pub const GRP_TXT: Self = Self(0x5);
    pub const GRP_DATA: Self = Self(0x6);
    pub const ANON_REQ: Self = Self(0x7);
    pub const PATH: Self = Self(0x8);
    pub const TRACE: Self = Self(0x9);
    pub const MULTIPART: Self = Self(0xA);
    pub const CONTROL: Self = Self(0xB);
    pub const RAW_CUSTOM: Self = Self(0xF);

    /// The low 4 bits of `v`.
    pub const fn new(v: u8) -> Self {
        Self(v & 0x0F)
    }
    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Header `0bVVPPPPRR`. The version is always 0 in a valid packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub route: Route,
    pub ptype: PayloadType,
}

impl Header {
    pub const fn new(route: Route, ptype: PayloadType) -> Self {
        Self { route, ptype }
    }
    pub const fn to_byte(self) -> u8 {
        (self.ptype.0 << 2) | self.route as u8
    }
    /// Reads a header byte. Any version other than 0 is rejected (`Dispatcher.cpp:153-156`).
    pub const fn from_byte(b: u8) -> Result<Self, Error> {
        if b >> 6 != 0 {
            return Err(Error::Version);
        }
        Ok(Self {
            route: Route::from_bits(b),
            ptype: PayloadType((b >> 2) & 0x0F),
        })
    }
}

/// The `path_len` byte: 6 bits of hop count, 2 bits of hash size minus 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PathLen(u8);

impl PathLen {
    /// No hops, 1-byte hashes.
    pub const EMPTY: Self = Self(0);
    /// "Unknown path" in contact state and the companion protocol; never over the air.
    pub const UNKNOWN: u8 = 0xFF;
    /// Largest encodable hop count.
    pub const MAX_HOPS: u8 = 63;

    /// Raw byte, validated as over the air: hash size 1 to 3, at most 64 bytes of path.
    pub const fn from_byte(b: u8) -> Result<Self, Error> {
        let p = Self(b);
        if b >> 6 == 3 {
            return Err(Error::PathHashSize);
        }
        if p.byte_len() > PATH_MAX {
            return Err(Error::PathTooLong);
        }
        Ok(p)
    }
    /// `hash_size` of 1 to 3 bytes, `hops` from 0 to 63, path of at most 64 bytes.
    pub const fn new(hash_size: u8, hops: u8) -> Result<Self, Error> {
        if hash_size == 0 || hash_size > 3 {
            return Err(Error::PathHashSize);
        }
        if hops > Self::MAX_HOPS {
            return Err(Error::PathTooLong);
        }
        Self::from_byte(((hash_size - 1) << 6) | hops)
    }
    pub const fn byte(self) -> u8 {
        self.0
    }
    pub const fn hash_size(self) -> u8 {
        (self.0 >> 6) + 1
    }
    pub const fn hops(self) -> u8 {
        self.0 & 63
    }
    pub const fn byte_len(self) -> usize {
        self.hops() as usize * self.hash_size() as usize
    }
    /// One more hop, if it fits (at most 63 hops, at most 64 bytes).
    pub const fn plus_one(self) -> Option<Self> {
        if self.hops() >= Self::MAX_HOPS {
            return None;
        }
        let p = Self(self.0 + 1);
        if p.byte_len() > PATH_MAX {
            None
        } else {
            Some(p)
        }
    }
}

/// A decoded packet, zero-copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet<'a> {
    pub header: Header,
    /// `[tc0, tc1]`, present only for routes 0 and 3.
    pub transport: Option<[u16; 2]>,
    pub path_len: PathLen,
    pub path: &'a [u8],
    pub payload: &'a [u8],
}

impl<'a> Packet<'a> {
    /// Decodes a received packet with the rules of `Dispatcher::tryParsePacket`
    /// (`Dispatcher.cpp:149-189`). An empty payload is accepted here.
    pub fn decode(raw: &'a [u8]) -> Result<Self, Error> {
        let mut r = mc_core::Reader::new(raw);
        let header = Header::from_byte(r.u8().map_err(|_| Error::Truncated)?)?;
        let transport = if header.route.has_transport_codes() {
            let t0 = r.u16_le().map_err(|_| Error::Truncated)?;
            let t1 = r.u16_le().map_err(|_| Error::Truncated)?;
            Some([t0, t1])
        } else {
            None
        };
        let path_len = PathLen::from_byte(r.u8().map_err(|_| Error::Truncated)?)?;
        let path = r.take(path_len.byte_len()).map_err(|_| Error::Truncated)?;
        let payload = r.rest();
        if payload.len() > PAYLOAD_MAX {
            return Err(Error::PayloadTooLong);
        }
        Ok(Self {
            header,
            transport,
            path_len,
            path,
            payload,
        })
    }

    /// Encoded length.
    pub const fn encoded_len(&self) -> usize {
        1 + if self.transport.is_some() { 4 } else { 0 } + 1 + self.path.len() + self.payload.len()
    }

    /// Encodes the packet. Rejects an inconsistent packet (transport codes without a route type
    /// that carries them, path of the wrong length, payload too long, packet over 255 bytes).
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        if self.transport.is_some() != self.header.route.has_transport_codes() {
            return Err(Error::Transport);
        }
        if self.path.len() != self.path_len.byte_len() {
            return Err(Error::PathMismatch);
        }
        if self.payload.len() > PAYLOAD_MAX {
            return Err(Error::PayloadTooLong);
        }
        if self.encoded_len() > mc_core::FRAME_MAX {
            return Err(Error::FrameTooLong);
        }
        let mut w = Writer::new(out);
        let res: Result<(), Overflow> = (|| {
            w.put_u8(self.header.to_byte())?;
            if let Some([t0, t1]) = self.transport {
                w.put_u16_le(t0)?;
                w.put_u16_le(t1)?;
            }
            w.put_u8(self.path_len.byte())?;
            w.put(self.path)?;
            w.put(self.payload)
        })();
        res.map_err(|_| Error::Overflow)?;
        Ok(w.position())
    }

    /// Duplicate-detection hash (`Packet.cpp:41-50`): `SHA256(type ‖ [path_len u16 if TRACE] ‖
    /// payload)[0..8]`.
    pub fn hash(&self) -> [u8; PACKET_HASH_LEN] {
        packet_hash(self.header.ptype, self.path_len, self.payload)
    }
}

/// Duplicate-detection hash of a payload (see [`Packet::hash`]).
pub fn packet_hash(ptype: PayloadType, path_len: PathLen, payload: &[u8]) -> [u8; PACKET_HASH_LEN] {
    let mut h = Sha256::new();
    h.update([ptype.value()]);
    if ptype == PayloadType::TRACE {
        // `path_len` is a uint16_t in the C++ class: two bytes, little-endian.
        h.update(u16::from(path_len.byte()).to_le_bytes());
    }
    h.update(payload);
    let full = h.finalize();
    let mut out = [0u8; PACKET_HASH_LEN];
    for (o, b) in out.iter_mut().zip(full.iter()) {
        *o = *b;
    }
    out
}

/// A packet under construction, in a fixed buffer (for transmission and tests).
#[derive(Debug, Clone)]
pub struct PacketBuf {
    pub header: Header,
    pub transport: Option<[u16; 2]>,
    pub path_len: PathLen,
    path: [u8; PATH_MAX],
    payload: [u8; PAYLOAD_MAX],
    payload_len: usize,
}

impl PacketBuf {
    /// Packet with no path and no transport codes.
    pub const fn new(route: Route, ptype: PayloadType) -> Self {
        Self {
            header: Header::new(route, ptype),
            transport: None,
            path_len: PathLen::EMPTY,
            path: [0; PATH_MAX],
            payload: [0; PAYLOAD_MAX],
            payload_len: 0,
        }
    }

    /// Copy of a decoded packet.
    pub fn from_packet(p: &Packet<'_>) -> Result<Self, Error> {
        let mut b = Self::new(p.header.route, p.header.ptype);
        b.transport = p.transport;
        b.set_path(p.path_len, p.path)?;
        b.set_payload(p.payload)?;
        Ok(b)
    }

    pub fn path(&self) -> &[u8] {
        self.path.get(..self.path_len.byte_len()).unwrap_or(&[])
    }

    pub fn payload(&self) -> &[u8] {
        self.payload.get(..self.payload_len).unwrap_or(&[])
    }

    /// The whole payload area, to fill in and then commit with [`PacketBuf::set_payload_len`].
    pub fn payload_buf(&mut self) -> &mut [u8; PAYLOAD_MAX] {
        &mut self.payload
    }

    pub fn set_payload_len(&mut self, n: usize) -> Result<(), Error> {
        if n > PAYLOAD_MAX {
            return Err(Error::PayloadTooLong);
        }
        self.payload_len = n;
        Ok(())
    }

    pub fn set_payload(&mut self, data: &[u8]) -> Result<(), Error> {
        let dst = self
            .payload
            .get_mut(..data.len())
            .ok_or(Error::PayloadTooLong)?;
        dst.copy_from_slice(data);
        self.payload_len = data.len();
        Ok(())
    }

    pub fn set_path(&mut self, path_len: PathLen, path: &[u8]) -> Result<(), Error> {
        if path.len() != path_len.byte_len() {
            return Err(Error::PathMismatch);
        }
        let dst = self.path.get_mut(..path.len()).ok_or(Error::PathTooLong)?;
        dst.copy_from_slice(path);
        self.path_len = path_len;
        Ok(())
    }

    /// Appends a hop hash at the end of the path (flood forwarding), at the path's hash size.
    pub fn push_hop(&mut self, hash: &[u8]) -> Result<(), Error> {
        let size = usize::from(self.path_len.hash_size());
        let next = self.path_len.plus_one().ok_or(Error::PathTooLong)?;
        let start = self.path_len.byte_len();
        let src = hash.get(..size).ok_or(Error::PathMismatch)?;
        let dst = self
            .path
            .get_mut(start..start + size)
            .ok_or(Error::PathTooLong)?;
        dst.copy_from_slice(src);
        self.path_len = next;
        Ok(())
    }

    /// Removes the first hash from the path (direct forwarding).
    pub fn pop_front_hop(&mut self) -> Result<(), Error> {
        let size = usize::from(self.path_len.hash_size());
        let hops = self.path_len.hops();
        if hops == 0 {
            return Err(Error::PathMismatch);
        }
        let len = self.path_len.byte_len();
        self.path.copy_within(size..len, 0);
        self.path_len = PathLen::new(self.path_len.hash_size(), hops - 1)?;
        Ok(())
    }

    pub fn as_packet(&self) -> Packet<'_> {
        Packet {
            header: self.header,
            transport: self.transport,
            path_len: self.path_len,
            path: self.path(),
            payload: self.payload(),
        }
    }

    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Error> {
        self.as_packet().encode(out)
    }

    pub fn hash(&self) -> [u8; PACKET_HASH_LEN] {
        packet_hash(self.header.ptype, self.path_len, self.payload())
    }
}
