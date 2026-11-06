//! Payloads (`docs/spec/air.md` §3): views over incoming data, plaintexts, and packet builders
//! that produce exactly the same packets as the C++.

use mc_core::Reader;

use crate::crypto::{self, ChannelSecret, LocalIdentity, PUB_KEY_LEN, SECRET_LEN, SIG_LEN};
use crate::packet::{PAYLOAD_MAX, PacketBuf, PathLen, PayloadType, Route};
use crate::{Error, advert};

/// Received payload, split up without decrypting, with the C++ minimum lengths
/// (`Mesh.cpp:41-332`). Shorter packets are ignored by the C++; here, `Err(Truncated)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload<'a> {
    /// REQ, RESPONSE, TXT_MSG, PATH: ≥ 5 bytes.
    Addressed {
        ptype: PayloadType,
        dest: u8,
        src: u8,
        sealed: &'a [u8],
    },
    /// ANON_REQ: ≥ 36 bytes.
    AnonReq {
        dest: u8,
        sender: &'a [u8; PUB_KEY_LEN],
        sealed: &'a [u8],
    },
    /// GRP_TXT, GRP_DATA: ≥ 4 bytes.
    Group {
        ptype: PayloadType,
        channel: u8,
        sealed: &'a [u8],
    },
    /// ACK: ≥ 4 bytes; only the first 4 matter.
    Ack {
        crc: u32,
        rest: &'a [u8],
    },
    /// ADVERT: ≥ 100 bytes.
    Advert(Advert<'a>),
    /// TRACE: ≥ 9 bytes (below that the C++ reads past the end of the packet; we reject).
    Trace {
        tag: u32,
        auth: u32,
        flags: u8,
        hashes: &'a [u8],
    },
    /// MULTIPART: ≥ 3 bytes.
    Multipart {
        remaining: u8,
        inner: PayloadType,
        body: &'a [u8],
    },
    /// CONTROL: ≥ 1 byte.
    Control {
        flags: u8,
        data: &'a [u8],
    },
    RawCustom(&'a [u8]),
    /// Types 0xC to 0xE: never forwarded.
    Unknown(&'a [u8]),
}

impl<'a> Payload<'a> {
    pub fn parse(ptype: PayloadType, p: &'a [u8]) -> Result<Self, Error> {
        let t = |e| -> Error {
            let _ = e;
            Error::Truncated
        };
        let mut r = Reader::new(p);
        Ok(match ptype {
            PayloadType::REQ | PayloadType::RESPONSE | PayloadType::TXT_MSG | PayloadType::PATH => {
                if p.len() < 5 {
                    return Err(Error::Truncated);
                }
                let dest = r.u8().map_err(t)?;
                let src = r.u8().map_err(t)?;
                Self::Addressed {
                    ptype,
                    dest,
                    src,
                    sealed: r.rest(),
                }
            }
            PayloadType::ANON_REQ => {
                if p.len() < 36 {
                    return Err(Error::Truncated);
                }
                let dest = r.u8().map_err(t)?;
                let sender = r.array::<PUB_KEY_LEN>().map_err(t)?;
                Self::AnonReq {
                    dest,
                    sender,
                    sealed: r.rest(),
                }
            }
            PayloadType::GRP_TXT | PayloadType::GRP_DATA => {
                if p.len() < 4 {
                    return Err(Error::Truncated);
                }
                let channel = r.u8().map_err(t)?;
                Self::Group {
                    ptype,
                    channel,
                    sealed: r.rest(),
                }
            }
            PayloadType::ACK => {
                let crc = r.u32_le().map_err(t)?;
                Self::Ack {
                    crc,
                    rest: r.rest(),
                }
            }
            PayloadType::ADVERT => Self::Advert(Advert::parse(p)?),
            PayloadType::TRACE => {
                let tag = r.u32_le().map_err(t)?;
                let auth = r.u32_le().map_err(t)?;
                let flags = r.u8().map_err(t)?;
                Self::Trace {
                    tag,
                    auth,
                    flags,
                    hashes: r.rest(),
                }
            }
            PayloadType::MULTIPART => {
                if p.len() < 3 {
                    return Err(Error::Truncated);
                }
                let b = r.u8().map_err(t)?;
                Self::Multipart {
                    remaining: b >> 4,
                    inner: PayloadType::new(b),
                    body: r.rest(),
                }
            }
            PayloadType::CONTROL => {
                let flags = r.u8().map_err(t)?;
                Self::Control {
                    flags,
                    data: r.rest(),
                }
            }
            PayloadType::RAW_CUSTOM => Self::RawCustom(p),
            _ => Self::Unknown(p),
        })
    }
}

/// Received advert: `pub(32) ‖ ts u32 ‖ sig(64) ‖ app_data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advert<'a> {
    pub pub_key: &'a [u8; PUB_KEY_LEN],
    pub ts: u32,
    pub sig: &'a [u8; SIG_LEN],
    /// Truncated to 32 bytes, as the C++ does before verification (`Mesh.cpp:269-281`).
    pub app_data: &'a [u8],
}

impl<'a> Advert<'a> {
    pub fn parse(p: &'a [u8]) -> Result<Self, Error> {
        let mut r = Reader::new(p);
        let pub_key = r.array::<PUB_KEY_LEN>().map_err(|_| Error::Truncated)?;
        let ts = r.u32_le().map_err(|_| Error::Truncated)?;
        let sig = r.array::<SIG_LEN>().map_err(|_| Error::Truncated)?;
        let rest = r.rest();
        let app_data = rest
            .get(..rest.len().min(advert::APP_DATA_MAX))
            .unwrap_or(&[]);
        Ok(Self {
            pub_key,
            ts,
            sig,
            app_data,
        })
    }

    /// Valid signature over `pub ‖ ts ‖ app_data`.
    pub fn verify(&self) -> bool {
        crypto::verify(
            self.pub_key,
            &[self.pub_key, &self.ts.to_le_bytes(), self.app_data],
            self.sig,
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Plaintexts (after `crypto::open`, zero padding included)

/// Decrypted TXT_MSG (`BaseChatMesh.cpp:231-300`): `ts ‖ flags ‖ text ‖ [0x00 ‖ attempt]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxtPlain<'a> {
    pub ts: u32,
    /// `txt_type << 2 | attempt & 3`.
    pub flags: u8,
    /// Up to the first NUL.
    pub text: &'a [u8],
    /// Byte following the terminating NUL: the extended attempt (> 3), or padding.
    pub after_nul: u8,
}

impl<'a> TxtPlain<'a> {
    /// The C++ requires more than 5 bytes (`len > 5`).
    pub fn parse(plain: &'a [u8]) -> Result<Self, Error> {
        if plain.len() <= 5 {
            return Err(Error::Truncated);
        }
        let mut r = Reader::new(plain);
        let ts = r.u32_le().map_err(|_| Error::Truncated)?;
        let flags = r.u8().map_err(|_| Error::Truncated)?;
        let rest = r.rest();
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        let text = rest.get(..end).unwrap_or(&[]);
        let after_nul = rest.get(end + 1).copied().unwrap_or(0);
        Ok(Self {
            ts,
            flags,
            text,
            after_nul,
        })
    }
    pub const fn txt_type(&self) -> u8 {
        self.flags >> 2
    }
    pub const fn attempt(&self) -> u8 {
        self.flags & 3
    }
    /// Ack CRC expected by the sender (`BaseChatMesh.cpp:245`).
    pub fn ack_crc(&self, sender_pub: &[u8; PUB_KEY_LEN]) -> u32 {
        crypto::ack_crc(self.ts, self.flags, self.text, sender_pub)
    }
}

/// Decrypted GRP_TXT: `ts ‖ txt_type ‖ "name: text"` (≥ 5 bytes, `txt_type >> 2 == 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupTxtPlain<'a> {
    pub ts: u32,
    pub txt_type: u8,
    /// Up to the first NUL.
    pub text: &'a [u8],
}

impl<'a> GroupTxtPlain<'a> {
    pub fn parse(plain: &'a [u8]) -> Result<Self, Error> {
        if plain.len() < 5 {
            return Err(Error::Truncated);
        }
        let mut r = Reader::new(plain);
        let ts = r.u32_le().map_err(|_| Error::Truncated)?;
        let txt_type = r.u8().map_err(|_| Error::Truncated)?;
        if txt_type >> 2 != 0 {
            return Err(Error::Unsupported);
        }
        let rest = r.rest();
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Ok(Self {
            ts,
            txt_type,
            text: rest.get(..end).unwrap_or(&[]),
        })
    }
}

/// Decrypted GRP_DATA: `data_type u16 ‖ length ‖ data` (≥ 3 bytes, length ≤ remainder).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupDataPlain<'a> {
    pub data_type: u16,
    pub data: &'a [u8],
}

impl<'a> GroupDataPlain<'a> {
    pub fn parse(plain: &'a [u8]) -> Result<Self, Error> {
        let mut r = Reader::new(plain);
        let data_type = r.u16_le().map_err(|_| Error::Truncated)?;
        let data = r.len8().map_err(|_| Error::Truncated)?;
        Ok(Self { data_type, data })
    }
}

/// Decrypted PATH: `path_len ‖ path ‖ extra_type ‖ extra` (`Mesh.cpp:160-172`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathPlain<'a> {
    pub path_len: PathLen,
    pub path: &'a [u8],
    /// Low nibble only (the upper 4 bits are reserved).
    pub extra_type: u8,
    /// The remainder, zero padding included.
    pub extra: &'a [u8],
}

impl<'a> PathPlain<'a> {
    pub fn parse(plain: &'a [u8]) -> Result<Self, Error> {
        let mut r = Reader::new(plain);
        let path_len = PathLen::from_byte(r.u8().map_err(|_| Error::Truncated)?)?;
        let path = r.take(path_len.byte_len()).map_err(|_| Error::Truncated)?;
        let extra_type = r.u8().map_err(|_| Error::Truncated)? & 0x0F;
        Ok(Self {
            path_len,
            path,
            extra_type,
            extra: r.rest(),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Builders (the route and path are set afterwards by the sender)

/// Addressed datagram (`Mesh::createDatagram`): `dest ‖ src ‖ seal(plain)`.
/// Allowed types: TXT_MSG, REQ, RESPONSE; rejected if `plain + 2 + 15 > 184`.
pub fn datagram(
    ptype: PayloadType,
    dest_hash: u8,
    src_hash: u8,
    secret: &[u8; SECRET_LEN],
    plain: &[u8],
) -> Result<PacketBuf, Error> {
    if !matches!(
        ptype,
        PayloadType::TXT_MSG | PayloadType::REQ | PayloadType::RESPONSE
    ) {
        return Err(Error::Unsupported);
    }
    if plain.len() + crypto::MAC_LEN + crypto::BLOCK_LEN - 1 > PAYLOAD_MAX {
        return Err(Error::PayloadTooLong);
    }
    addressed(ptype, dest_hash, src_hash, secret, plain)
}

fn addressed(
    ptype: PayloadType,
    dest_hash: u8,
    src_hash: u8,
    secret: &[u8; SECRET_LEN],
    plain: &[u8],
) -> Result<PacketBuf, Error> {
    let mut b = PacketBuf::new(Route::Flood, ptype);
    let buf = b.payload_buf();
    let (head, tail) = buf.split_at_mut(2);
    head.copy_from_slice(&[dest_hash, src_hash]);
    let n = crypto::seal(secret, plain, tail)?;
    b.set_payload_len(2 + n)?;
    Ok(b)
}

/// TXT_MSG plaintext (`composeMsgPacket`, `sendCommandData`) into `out`; returns its length and
/// the expected ack CRC (computed with the sender's key, without the extended-attempt tail).
/// Text of at most 160 bytes, 158 if `attempt > 3`.
pub fn txt_plain(
    ts: u32,
    txt_type: u8,
    attempt: u8,
    text: &[u8],
    sender_pub: &[u8; PUB_KEY_LEN],
    out: &mut [u8],
) -> Result<(usize, u32), Error> {
    const MAX_TEXT: usize = 160;
    if text.len() > MAX_TEXT || (attempt > 3 && text.len() > MAX_TEXT - 2) || text.contains(&0) {
        return Err(Error::TextTooLong);
    }
    let flags = (txt_type << 2) | (attempt & 3);
    let mut w = mc_core::Writer::new(out);
    let res = (|| {
        w.put_u32_le(ts)?;
        w.put_u8(flags)?;
        w.put(text)?;
        if attempt > 3 {
            w.put_u8(0)?;
            w.put_u8(attempt)?;
        }
        Ok::<(), mc_core::Overflow>(())
    })();
    res.map_err(|_| Error::Overflow)?;
    Ok((w.position(), crypto::ack_crc(ts, flags, text, sender_pub)))
}

/// Anonymous datagram (`createAnonDatagram`): `dest ‖ sender_pub ‖ seal(plain)`.
pub fn anon_datagram(
    dest_hash: u8,
    sender: &LocalIdentity,
    secret: &[u8; SECRET_LEN],
    plain: &[u8],
) -> Result<PacketBuf, Error> {
    if plain.len() + 1 + PUB_KEY_LEN + crypto::BLOCK_LEN - 1 > PAYLOAD_MAX {
        return Err(Error::PayloadTooLong);
    }
    let mut b = PacketBuf::new(Route::Flood, PayloadType::ANON_REQ);
    let buf = b.payload_buf();
    let (head, tail) = buf.split_at_mut(1 + PUB_KEY_LEN);
    let (d, k) = head.split_at_mut(1);
    d.copy_from_slice(&[dest_hash]);
    k.copy_from_slice(sender.pub_key());
    let n = crypto::seal(secret, plain, tail)?;
    b.set_payload_len(1 + PUB_KEY_LEN + n)?;
    Ok(b)
}

/// Channel datagram (`createGroupDatagram`): `hash ‖ seal(plain)`; rejected if
/// `plain + 1 + 15 > 184`.
pub fn group_datagram(
    ptype: PayloadType,
    channel: &ChannelSecret,
    plain: &[u8],
) -> Result<PacketBuf, Error> {
    if !matches!(ptype, PayloadType::GRP_TXT | PayloadType::GRP_DATA) {
        return Err(Error::Unsupported);
    }
    if plain.len() + 1 + crypto::BLOCK_LEN - 1 > PAYLOAD_MAX {
        return Err(Error::PayloadTooLong);
    }
    let mut b = PacketBuf::new(Route::Flood, ptype);
    let buf = b.payload_buf();
    let (head, tail) = buf.split_at_mut(1);
    head.copy_from_slice(&[channel.hash()]);
    let n = crypto::seal(&channel.0, plain, tail)?;
    b.set_payload_len(1 + n)?;
    Ok(b)
}

/// GRP_TXT plaintext (`sendGroupMessage`): `ts ‖ 0 ‖ "name: text"`, truncated to 160 bytes of
/// text, prefix included. Returns the length.
pub fn group_txt_plain(
    ts: u32,
    sender_name: &[u8],
    text: &[u8],
    out: &mut [u8],
) -> Result<usize, Error> {
    const MAX_TEXT: usize = 160;
    let mut w = mc_core::Writer::new(out);
    let prefix_len = sender_name.len() + 2;
    let text_len = text.len().min(MAX_TEXT.saturating_sub(prefix_len));
    let res = (|| {
        w.put_u32_le(ts)?;
        w.put_u8(0)?;
        w.put(sender_name)?;
        w.put(b": ")?;
        w.put(text.get(..text_len).unwrap_or(&[]))?;
        Ok::<(), mc_core::Overflow>(())
    })();
    res.map_err(|_| Error::Overflow)?;
    Ok(w.position())
}

/// GRP_DATA plaintext (`sendGroupData`): `data_type ‖ length ‖ data` (≤ 165).
pub fn group_data_plain(data_type: u16, data: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    const MAX_GROUP_DATA: usize = PAYLOAD_MAX - crypto::BLOCK_LEN - 3;
    if data.len() > MAX_GROUP_DATA {
        return Err(Error::PayloadTooLong);
    }
    let mut w = mc_core::Writer::new(out);
    w.put_u16_le(data_type).map_err(|_| Error::Overflow)?;
    w.put_len8(data).map_err(|_| Error::Overflow)?;
    Ok(w.position())
}

/// PATH (`createPathReturn`): `dest ‖ src ‖ seal(path_len ‖ path ‖ extra_type ‖ extra)`;
/// without extra, `0xFF ‖ random(4)`. Rejected if `path bytes + extra + 5 > 166`.
pub fn path_return(
    dest_hash: u8,
    src_hash: u8,
    secret: &[u8; SECRET_LEN],
    path_len: PathLen,
    path: &[u8],
    extra: Option<(u8, &[u8])>,
    random4: [u8; 4],
) -> Result<PacketBuf, Error> {
    const MAX_COMBINED_PATH: usize = PAYLOAD_MAX - 2 - crypto::BLOCK_LEN;
    if path.len() != path_len.byte_len() {
        return Err(Error::PathMismatch);
    }
    let extra_len = extra.map_or(0, |(_, e)| e.len());
    if path.len() + extra_len + 5 > MAX_COMBINED_PATH {
        return Err(Error::PayloadTooLong);
    }
    let mut plain = [0u8; PAYLOAD_MAX];
    let mut w = mc_core::Writer::new(&mut plain);
    let res = (|| {
        w.put_u8(path_len.byte())?;
        w.put(path)?;
        match extra {
            Some((t, e)) if !e.is_empty() => {
                w.put_u8(t)?;
                w.put(e)?;
            }
            _ => {
                w.put_u8(0xFF)?;
                w.put(&random4)?;
            }
        }
        Ok::<(), mc_core::Overflow>(())
    })();
    res.map_err(|_| Error::Overflow)?;
    let n = w.position();
    addressed(
        PayloadType::PATH,
        dest_hash,
        src_hash,
        secret,
        plain.get(..n).unwrap_or(&[]),
    )
}

/// ACK (`createAck`): the ack bytes as is (4, 5 or 6).
pub fn ack(bytes: &[u8]) -> Result<PacketBuf, Error> {
    let mut b = PacketBuf::new(Route::Flood, PayloadType::ACK);
    b.set_payload(bytes)?;
    Ok(b)
}

/// MULTIPART-ACK (`createMultiAck`): `(remaining << 4 | ACK) ‖ ack`.
pub fn multi_ack(bytes: &[u8], remaining: u8) -> Result<PacketBuf, Error> {
    let mut b = PacketBuf::new(Route::Flood, PayloadType::MULTIPART);
    let buf = b.payload_buf();
    let (head, tail) = buf.split_at_mut(1);
    head.copy_from_slice(&[(remaining << 4) | PayloadType::ACK.value()]);
    tail.get_mut(..bytes.len())
        .ok_or(Error::PayloadTooLong)?
        .copy_from_slice(bytes);
    b.set_payload_len(1 + bytes.len())?;
    Ok(b)
}

/// Signed advert (`createAdvert`): `pub ‖ ts ‖ sig ‖ app_data`, signature over
/// `pub ‖ ts ‖ app_data`.
pub fn advert(id: &LocalIdentity, ts: u32, app_data: &[u8]) -> Result<PacketBuf, Error> {
    if app_data.len() > advert::APP_DATA_MAX {
        return Err(Error::PayloadTooLong);
    }
    let ts_b = ts.to_le_bytes();
    let sig = id.sign(&[id.pub_key(), &ts_b, app_data])?;
    let mut b = PacketBuf::new(Route::Flood, PayloadType::ADVERT);
    let mut w = mc_core::Writer::new(b.payload_buf());
    let res = (|| {
        w.put(id.pub_key())?;
        w.put(&ts_b)?;
        w.put(&sig)?;
        w.put(app_data)?;
        Ok::<(), mc_core::Overflow>(())
    })();
    res.map_err(|_| Error::Overflow)?;
    let n = w.position();
    b.set_payload_len(n)?;
    Ok(b)
}
