//! Human-readable description of an air frame, decrypted when we have the key.

use mc_air::crypto::{self, ChannelSecret, LocalIdentity};
use mc_air::payload::{Advert, GroupDataPlain, GroupTxtPlain, PathPlain, Payload, TxtPlain};
use mc_air::{AppData, Packet, PayloadType};
use mc_core::Hex;

/// What we can decrypt.
#[derive(Default)]
pub struct Keys {
    pub identities: Vec<(String, LocalIdentity)>,
    /// Known public keys (contacts), for direct messages to our identities.
    pub peers: Vec<[u8; 32]>,
    pub channels: Vec<(String, ChannelSecret)>,
}

fn type_name(t: PayloadType) -> &'static str {
    match t.value() {
        0x0 => "REQ",
        0x1 => "RESPONSE",
        0x2 => "TXT_MSG",
        0x3 => "ACK",
        0x4 => "ADVERT",
        0x5 => "GRP_TXT",
        0x6 => "GRP_DATA",
        0x7 => "ANON_REQ",
        0x8 => "PATH",
        0x9 => "TRACE",
        0xA => "MULTIPART",
        0xB => "CONTROL",
        0xF => "RAW_CUSTOM",
        _ => "?",
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// One line describing the frame `raw`.
pub fn describe(raw: &[u8], keys: &Keys) -> String {
    let p = match Packet::decode(raw) {
        Ok(p) => p,
        Err(e) => return format!("unreadable ({e:?}) {}", Hex(raw)),
    };
    let mut s = format!(
        "{:?} {} hops={}x{}B{} hash={}",
        p.header.route,
        type_name(p.header.ptype),
        p.path_len.hops(),
        p.path_len.hash_size(),
        p.transport
            .map(|t| format!(" tc={:04x}", t[0]))
            .unwrap_or_default(),
        Hex(&p.hash()),
    );
    let Ok(pl) = Payload::parse(p.header.ptype, p.payload) else {
        s.push_str(&format!(" unreadable payload {}", Hex(p.payload)));
        return s;
    };
    match pl {
        Payload::Advert(a) => s.push_str(&advert(&a)),
        Payload::Group {
            ptype,
            channel,
            sealed,
        } => {
            s.push_str(&format!(" ch=#{channel:02x}"));
            for (name, secret) in keys.channels.iter().filter(|(_, c)| c.hash() == channel) {
                let mut buf = [0u8; 200];
                if let Ok(plain) = crypto::open(&secret.0, sealed, &mut buf) {
                    if ptype == PayloadType::GRP_TXT {
                        if let Ok(g) = GroupTxtPlain::parse(plain) {
                            s.push_str(&format!(" [{name}] ts={} \"{}\"", g.ts, text(g.text)));
                        }
                    } else if let Ok(g) = GroupDataPlain::parse(plain) {
                        s.push_str(&format!(
                            " [{name}] type={:04x} data={}",
                            g.data_type,
                            Hex(g.data)
                        ));
                    }
                }
            }
        }
        Payload::Addressed {
            ptype,
            dest,
            src,
            sealed,
        } => {
            s.push_str(&format!(" {src:02x}→{dest:02x}"));
            for (name, id) in keys.identities.iter().filter(|(_, id)| id.hash() == dest) {
                for peer in keys.peers.iter().filter(|k| k[0] == src) {
                    let Some(secret) = id.shared_secret(peer) else {
                        continue;
                    };
                    let mut buf = [0u8; 200];
                    let Ok(plain) = crypto::open(&secret, sealed, &mut buf) else {
                        continue;
                    };
                    s.push_str(&format!(" [to {name}, from {}]", Hex(&peer[..4])));
                    if ptype == PayloadType::TXT_MSG {
                        if let Ok(t) = TxtPlain::parse(plain) {
                            s.push_str(&format!(
                                " ts={} type={} attempt={} \"{}\" ack={:08x}",
                                t.ts,
                                t.txt_type(),
                                t.attempt(),
                                text(t.text),
                                t.ack_crc(peer)
                            ));
                        }
                    } else if ptype == PayloadType::PATH {
                        if let Ok(pp) = PathPlain::parse(plain) {
                            s.push_str(&format!(
                                " path={:02x}:{} extra={} {}",
                                pp.path_len.byte(),
                                Hex(pp.path),
                                pp.extra_type,
                                Hex(pp.extra)
                            ));
                        }
                    } else {
                        s.push_str(&format!(" plain={}", Hex(plain)));
                    }
                }
            }
        }
        Payload::Ack { crc, rest } => s.push_str(&format!(" ack={crc:08x} rest={}", Hex(rest))),
        Payload::AnonReq { dest, sender, .. } => {
            s.push_str(&format!(" from {} → {dest:02x}", Hex(&sender[..4])))
        }
        Payload::Trace {
            tag, flags, hashes, ..
        } => s.push_str(&format!(
            " tag={tag:08x} flags={flags} hashes={} snr={}",
            Hex(hashes),
            Hex(p.path)
        )),
        Payload::Control { flags, data } => {
            s.push_str(&format!(" flags={flags:02x} data={}", Hex(data)))
        }
        Payload::Multipart {
            remaining,
            inner,
            body,
        } => s.push_str(&format!(
            " remaining={remaining} type={} {}",
            type_name(inner),
            Hex(body)
        )),
        Payload::RawCustom(b) | Payload::Unknown(b) => s.push_str(&format!(" {}", Hex(b))),
    }
    s
}

pub fn advert(a: &Advert<'_>) -> String {
    let ok = if a.verify() {
        "signature ok"
    } else {
        "BAD SIGNATURE"
    };
    match AppData::decode(a.app_data) {
        Ok(ad) => format!(
            " key={} ts={} kind={} name=\"{}\"{} {ok}",
            Hex(&a.pub_key[..4]),
            a.ts,
            ad.kind,
            text(ad.name_str()),
            ad.location
                .map(|(la, lo)| format!(" pos={la},{lo}"))
                .unwrap_or_default(),
        ),
        Err(e) => format!(" key={} unreadable data ({e:?}) {ok}", Hex(&a.pub_key[..4])),
    }
}
