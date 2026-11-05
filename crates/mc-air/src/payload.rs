//! Charges utiles (`docs/spec/air.md` §3) : vues sur ce qui arrive, textes clairs, et
//! constructeurs de trames identiques à celles du C++.

use mc_core::Reader;

use crate::crypto::{self, ChannelSecret, LocalIdentity, PUB_KEY_LEN, SECRET_LEN, SIG_LEN};
use crate::packet::{PAYLOAD_MAX, PacketBuf, PathLen, PayloadType, Route};
use crate::{Error, advert};

/// Charge utile reçue, découpée sans déchiffrement, avec les longueurs minimales du C++
/// (`Mesh.cpp:41-332`). Les trames plus courtes sont ignorées par le C++ ; ici, `Err(Truncated)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload<'a> {
    /// REQ, RESPONSE, TXT_MSG, PATH : ≥ 5 octets.
    Addressed {
        ptype: PayloadType,
        dest: u8,
        src: u8,
        sealed: &'a [u8],
    },
    /// ANON_REQ : ≥ 36 octets.
    AnonReq {
        dest: u8,
        sender: &'a [u8; PUB_KEY_LEN],
        sealed: &'a [u8],
    },
    /// GRP_TXT, GRP_DATA : ≥ 4 octets.
    Group {
        ptype: PayloadType,
        channel: u8,
        sealed: &'a [u8],
    },
    /// ACK : ≥ 4 octets ; seuls les 4 premiers comptent.
    Ack {
        crc: u32,
        rest: &'a [u8],
    },
    /// ADVERT : ≥ 100 octets.
    Advert(Advert<'a>),
    /// TRACE : ≥ 9 octets (le C++ lit hors de la trame en dessous ; nous refusons).
    Trace {
        tag: u32,
        auth: u32,
        flags: u8,
        hashes: &'a [u8],
    },
    /// MULTIPART : ≥ 3 octets.
    Multipart {
        remaining: u8,
        inner: PayloadType,
        body: &'a [u8],
    },
    /// CONTROL : ≥ 1 octet.
    Control {
        flags: u8,
        data: &'a [u8],
    },
    RawCustom(&'a [u8]),
    /// Types 0xC à 0xE : jamais relayés.
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

/// Annonce reçue : `pub(32) ‖ ts u32 ‖ sig(64) ‖ app_data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advert<'a> {
    pub pub_key: &'a [u8; PUB_KEY_LEN],
    pub ts: u32,
    pub sig: &'a [u8; SIG_LEN],
    /// Tronqué à 32 octets, comme en C++ avant la vérification (`Mesh.cpp:269-281`).
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

    /// Signature valide sur `pub ‖ ts ‖ app_data`.
    pub fn verify(&self) -> bool {
        crypto::verify(
            self.pub_key,
            &[self.pub_key, &self.ts.to_le_bytes(), self.app_data],
            self.sig,
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Textes clairs (après `crypto::open`, bourrage de zéros compris)

/// TXT_MSG déchiffré (`BaseChatMesh.cpp:231-300`) : `ts ‖ flags ‖ texte ‖ [0x00 ‖ essai]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxtPlain<'a> {
    pub ts: u32,
    /// `txt_type << 2 | essai & 3`.
    pub flags: u8,
    /// Jusqu'au premier NUL.
    pub text: &'a [u8],
    /// Octet qui suit le NUL de fin : l'essai étendu (> 3), ou du bourrage.
    pub after_nul: u8,
}

impl<'a> TxtPlain<'a> {
    /// Le C++ exige plus de 5 octets (`len > 5`).
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
    /// CRC d'accusé attendu par l'émetteur (`BaseChatMesh.cpp:245`).
    pub fn ack_crc(&self, sender_pub: &[u8; PUB_KEY_LEN]) -> u32 {
        crypto::ack_crc(self.ts, self.flags, self.text, sender_pub)
    }
}

/// GRP_TXT déchiffré : `ts ‖ txt_type ‖ "nom: texte"` (≥ 5 octets, `txt_type >> 2 == 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupTxtPlain<'a> {
    pub ts: u32,
    pub txt_type: u8,
    /// Jusqu'au premier NUL.
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

/// GRP_DATA déchiffré : `data_type u16 ‖ longueur ‖ données` (≥ 3 octets, longueur ≤ reste).
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

/// PATH déchiffré : `path_len ‖ chemin ‖ type_extra ‖ extra` (`Mesh.cpp:160-172`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathPlain<'a> {
    pub path_len: PathLen,
    pub path: &'a [u8],
    /// Quartet bas seulement (les 4 bits hauts sont réservés).
    pub extra_type: u8,
    /// Le reste, bourrage de zéros compris.
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
// Constructeurs (la route et le chemin sont fixés ensuite par l'émetteur)

/// Datagramme adressé (`Mesh::createDatagram`) : `dest ‖ src ‖ seal(clair)`.
/// Types permis : TXT_MSG, REQ, RESPONSE ; refusé si `clair + 2 + 15 > 184`.
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

/// Texte clair d'un TXT_MSG (`composeMsgPacket`, `sendCommandData`) dans `out` ; renvoie sa
/// longueur et le CRC d'accusé attendu (calculé avec la clé de l'émetteur, sans la queue
/// d'essai étendu). Texte de 160 octets au plus, 158 si `attempt > 3`.
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

/// Datagramme anonyme (`createAnonDatagram`) : `dest ‖ pub_émetteur ‖ seal(clair)`.
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

/// Datagramme de canal (`createGroupDatagram`) : `hash ‖ seal(clair)` ; refusé si
/// `clair + 1 + 15 > 184`.
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

/// Texte clair d'un GRP_TXT (`sendGroupMessage`) : `ts ‖ 0 ‖ "nom: texte"`, le tout tronqué à
/// 160 octets de texte préfixe compris. Renvoie la longueur.
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

/// Texte clair d'un GRP_DATA (`sendGroupData`) : `data_type ‖ longueur ‖ données` (≤ 165).
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

/// PATH (`createPathReturn`) : `dest ‖ src ‖ seal(path_len ‖ chemin ‖ type_extra ‖ extra)` ;
/// sans extra, `0xFF ‖ aléa(4)`. Refusé si `octets de chemin + extra + 5 > 166`.
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

/// ACK (`createAck`) : les octets d'accusé tels quels (4, 5 ou 6).
pub fn ack(bytes: &[u8]) -> Result<PacketBuf, Error> {
    let mut b = PacketBuf::new(Route::Flood, PayloadType::ACK);
    b.set_payload(bytes)?;
    Ok(b)
}

/// MULTIPART-ACK (`createMultiAck`) : `(restants << 4 | ACK) ‖ accusé`.
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

/// Annonce signée (`createAdvert`) : `pub ‖ ts ‖ sig ‖ app_data`, signature sur
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
