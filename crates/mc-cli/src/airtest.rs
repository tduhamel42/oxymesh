//! `mc airtest` : éprouve `mc-air` contre un vrai firmware MeshCore C++, sans rien inonder.
//!
//! La référence (`--ref`, un compagnon USB) émet nos trames brutes (`SEND_RAW_PACKET`) et nous
//! rapporte tout ce qu'elle entend (`LOG_RX_DATA`). L'appareil sous test (`--dut`, un compagnon
//! USB en C++) doit accepter, déchiffrer et accuser nos trames ; ses propres trames, captées par
//! la référence, doivent se décoder avec notre code. Tout passe en zéro saut (route directe,
//! chemin vide) : aucun relais ne les retransmet.

use mc_air::crypto::{self, ChannelSecret, LocalIdentity};
use mc_air::payload::{self, Advert, GroupDataPlain, Payload, TxtPlain};
use mc_air::{AppData, Packet, PacketBuf, PayloadType, Route};
use mc_core::Hex;
use mc_proto::codec::{APP_VER, fixed_str, to_fixed};
use mc_proto::{Command, Response};

use crate::port::{Port, now_unix};

const ALICE_NAME: &str = "oxymesh-alice";
const CHANNEL: &str = "#oxymesh-test";

struct Report {
    ok: usize,
    failed: usize,
}

impl Report {
    fn check(&mut self, what: &str, res: Result<String, String>) {
        match res {
            Ok(detail) => {
                self.ok += 1;
                println!(
                    "✓ {what}{}",
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(" — {detail}")
                    }
                );
            }
            Err(e) => {
                self.failed += 1;
                println!("✗ {what} — {e}");
            }
        }
    }
}

fn alice() -> LocalIdentity {
    let mut i = 0u32;
    loop {
        let seed: [u8; 32] = crypto::sha256_trunc(&[b"oxymesh-airtest-alice-", &i.to_le_bytes()]);
        let id = LocalIdentity::from_seed(&seed);
        if !id.is_reserved() {
            return id;
        }
        i += 1;
    }
}

/// Encode une trame en zéro saut (route directe, chemin vide).
fn zero_hop(mut b: PacketBuf) -> Result<Vec<u8>, String> {
    b.header.route = Route::Direct;
    let mut out = [0u8; 255];
    let n = b.encode(&mut out).map_err(|e| format!("{e:?}"))?;
    Ok(out[..n].to_vec())
}

fn inject(r: &mut Port, raw: &[u8]) -> Result<(), String> {
    match Response::decode(&r.ask(&Command::SendRawPacket { priority: 0, raw })?) {
        Ok(Response::Ok) => Ok(()),
        other => Err(format!("SEND_RAW_PACKET refusé : {other:?}")),
    }
}

/// Attend une trame entendue par la référence qui satisfait `pred`.
fn heard<T>(
    r: &mut Port,
    secs: u64,
    mut pred: impl FnMut(&Packet<'_>) -> Option<T>,
) -> Result<T, String> {
    r.wait(secs, |resp| match resp {
        Response::LogRxData { raw, .. } => Packet::decode(raw).ok().and_then(|p| pred(&p)),
        _ => None,
    })
}

fn expect_ok(frame: &[u8], what: &str) -> Result<(), String> {
    match Response::decode(frame) {
        Ok(Response::Ok) => Ok(()),
        other => Err(format!("{what} : {other:?}")),
    }
}

/// Récupère les messages en attente de l'appareil jusqu'à trouver celui que `pred` reconnaît.
/// Les autres (trafic réel) sont comptés, pas affichés.
fn fetch<T>(d: &mut Port, mut pred: impl FnMut(&Response<'_>) -> Option<T>) -> Result<T, String> {
    let _ = d.wait(15, |r| matches!(r, Response::MsgWaiting).then_some(()));
    let mut skipped = 0;
    for _ in 0..300 {
        let f = d.ask(&Command::SyncNextMessage)?;
        let r = Response::decode(&f).map_err(|e| format!("{e:?}"))?;
        if matches!(r, Response::NoMoreMessages) {
            break;
        }
        if let Some(v) = pred(&r) {
            return Ok(v);
        }
        skipped += 1;
    }
    Err(format!(
        "message absent de la file ({skipped} autres messages ignorés)"
    ))
}

pub fn run(ref_path: &str, dut_path: &str, verbose: bool) -> Result<(), String> {
    let mut rf = Port::open(ref_path, "ref")?;
    let mut d = Port::open(dut_path, "dut")?;
    rf.verbose = verbose;
    d.verbose = verbose;
    let mut rep = Report { ok: 0, failed: 0 };

    // Mise en route des deux appareils.
    for p in [&mut rf, &mut d] {
        let mut ok = false;
        for _ in 0..3 {
            if p.ask(&Command::DeviceQuery { app_ver: APP_VER }).is_ok() {
                ok = true;
                break;
            }
        }
        if !ok {
            return Err(format!("{} ne répond pas", p.name));
        }
    }
    let _ = rf.ask(&Command::AppStart {
        reserved: [0; 7],
        app_name: b"oxymesh",
    })?;
    let self_info = d.ask(&Command::AppStart {
        reserved: [0; 7],
        app_name: b"oxymesh",
    })?;
    let (dut_key, dut_name) = match Response::decode(&self_info) {
        Ok(Response::SelfInfo(si)) => (si.key, si.node_name.to_vec()),
        other => return Err(format!("SELF_INFO attendu : {other:?}")),
    };
    println!(
        "appareil sous test : {} « {} »",
        Hex(&dut_key[..4]),
        String::from_utf8_lossy(&dut_name)
    );
    let now = now_unix();
    let _ = d.ask(&Command::SetDeviceTime { unix_s: now });
    let a = alice();
    let s_ad = a
        .shared_secret(&dut_key)
        .ok_or("clé de l'appareil illisible")?;
    println!(
        "identité de test : {} « {ALICE_NAME} »",
        Hex(&a.pub_key()[..4])
    );

    // 1. Notre annonce, signée par mc-air, acceptée par le C++.
    let ts_adv = now;
    rep.check(
        "annonce de mc-air acceptée par le C++",
        (|| {
            let ad = AppData {
                kind: mc_air::advert::TYPE_CHAT,
                name: Some(ALICE_NAME.as_bytes()),
                ..Default::default()
            };
            let mut buf = [0u8; 32];
            let n = ad.encode(&mut buf).map_err(|e| format!("{e:?}"))?;
            let raw =
                zero_hop(payload::advert(&a, ts_adv, &buf[..n]).map_err(|e| format!("{e:?}"))?)?;
            inject(&mut rf, &raw)?;
            let key = *a.pub_key();
            d.wait(10, |r| match r {
                Response::Advert { key: k } if *k == key => {
                    Some("contact ajouté (ADVERT)".to_string())
                }
                Response::NewAdvert(c) if c.key == key => Some(format!(
                    "vue (NEW_ADVERT, nom « {} »)",
                    String::from_utf8_lossy(fixed_str(&c.name))
                )),
                _ => None,
            })
        })(),
    );

    // Contact en zéro saut : les réponses de l'appareil restent locales.
    let add = Command::AddUpdateContact {
        key: *a.pub_key(),
        kind: 1,
        flags: 0,
        path_len: 0,
        path: [0; 64],
        name: to_fixed::<32>(ALICE_NAME.as_bytes()),
        last_advert: ts_adv,
        lat_lon: None,
        lastmod: None,
    };
    expect_ok(&d.ask(&add)?, "ADD_UPDATE_CONTACT")?;

    // 2. Message direct de mc-air, lu par le C++, qui renvoie le bon accusé.
    let text = format!("oxymesh airtest {now} : ça marche ?");
    let ts_dm = now;
    let mut plain = [0u8; 200];
    let (n, ack) = payload::txt_plain(ts_dm, 0, 0, text.as_bytes(), a.pub_key(), &mut plain)
        .map_err(|e| format!("{e:?}"))?;
    let raw = zero_hop(
        payload::datagram(
            PayloadType::TXT_MSG,
            dut_key[0],
            a.hash(),
            &s_ad,
            &plain[..n],
        )
        .map_err(|e| format!("{e:?}"))?,
    )?;
    let _ = rf.drain()?;
    inject(&mut rf, &raw)?;
    rep.check(
        "message direct de mc-air déchiffré par le C++",
        fetch(&mut d, |r| match r {
            Response::ContactMsg {
                prefix,
                text: t,
                timestamp,
                txt_type,
                path_len,
                ..
            } if prefix[..] == a.pub_key()[..6] => Some(
                if *t == text.as_bytes() && *timestamp == ts_dm && *txt_type == 0 {
                    Ok(format!("texte identique, path_len={path_len:#04x}"))
                } else {
                    Err(format!(
                        "reçu « {} » ts={timestamp}",
                        String::from_utf8_lossy(t)
                    ))
                },
            ),
            _ => None,
        })
        .and_then(|x| x),
    );
    rep.check(
        "accusé du C++ égal au CRC calculé par mc-air",
        heard(&mut rf, 10, |p| {
            match Payload::parse(p.header.ptype, p.payload) {
                Ok(Payload::Ack { crc, rest }) if crc == ack => {
                    Some(format!("{crc:08x}, {} octets", 4 + rest.len()))
                }
                _ => None,
            }
        }),
    );

    // 3. Message direct du C++, déchiffré par mc-air ; son accusé, calculé par mc-air, confirme l'envoi.
    let reply = format!("réponse du C++ {now}");
    let ts_reply = now_unix().max(now + 1);
    let sent = d.ask(&Command::SendTxtMsg {
        txt_type: 0,
        attempt: 0,
        timestamp: ts_reply,
        prefix: a.pub_key()[..6].try_into().unwrap_or([0; 6]),
        text: reply.as_bytes(),
    })?;
    let expected_ack = match Response::decode(&sent) {
        Ok(Response::Sent { flood, ack, .. }) => {
            rep.check(
                "SEND_TXT_MSG en direct",
                if flood {
                    Err("envoyé en inondation".into())
                } else {
                    Ok(String::new())
                },
            );
            Some(ack)
        }
        other => {
            rep.check("SEND_TXT_MSG", Err(format!("{other:?}")));
            None
        }
    };
    let got = heard(&mut rf, 10, |p| {
        match Payload::parse(p.header.ptype, p.payload) {
            Ok(Payload::Addressed {
                ptype: PayloadType::TXT_MSG,
                dest,
                src,
                sealed,
            }) if dest == a.hash() && src == dut_key[0] => {
                let mut buf = [0u8; 200];
                let plain = crypto::open(&s_ad, sealed, &mut buf).ok()?;
                let t = TxtPlain::parse(plain).ok()?;
                Some((t.text.to_vec(), t.ts, t.ack_crc(&dut_key)))
            }
            _ => None,
        }
    });
    rep.check(
        "message direct du C++ déchiffré par mc-air",
        match &got {
            Ok((t, ts, _)) if *t == reply.as_bytes() && *ts == ts_reply => {
                Ok(format!("« {reply} »"))
            }
            Ok((t, ts, _)) => Err(format!("« {} » ts={ts}", String::from_utf8_lossy(t))),
            Err(e) => Err(e.clone()),
        },
    );
    if let (Ok((_, _, ours)), Some(theirs)) = (&got, expected_ack) {
        rep.check(
            "CRC d'accusé de mc-air égal à celui annoncé par le C++",
            if ours.to_le_bytes() == theirs {
                Ok(format!("{ours:08x}"))
            } else {
                Err(format!("mc-air {ours:08x}, C++ {}", Hex(&theirs)))
            },
        );
        let mut ack6 = [0u8; 6];
        ack6[..4].copy_from_slice(&theirs);
        ack6[5] = 0x5a;
        let raw = zero_hop(payload::ack(&ack6).map_err(|e| format!("{e:?}"))?)?;
        inject(&mut rf, &raw)?;
        rep.check(
            "accusé de mc-air reconnu par le C++ (SEND_CONFIRMED)",
            d.wait(10, |r| match r {
                Response::SendConfirmed { ack, rtt_ms } if *ack == theirs => {
                    Some(format!("aller-retour {rtt_ms} ms"))
                }
                _ => None,
            }),
        );
    }

    // 4. Annonce du C++, vérifiée par mc-air.
    expect_ok(
        &d.ask(&Command::SendSelfAdvert { flood: Some(false) })?,
        "SEND_SELF_ADVERT",
    )?;
    rep.check(
        "annonce du C++ vérifiée par mc-air",
        heard(&mut rf, 10, |p| {
            match Payload::parse(p.header.ptype, p.payload) {
                Ok(Payload::Advert(adv)) if *adv.pub_key == dut_key => {
                    Some(adv_check(&adv, &dut_name))
                }
                _ => None,
            }
        })
        .and_then(|x| x),
    );

    // 5. Canal privé de test : datagramme du C++ lu par mc-air, texte de mc-air lu par le C++.
    let chan = ChannelSecret::from_hashtag(CHANNEL);
    let mut slot = None;
    for idx in (1..40u8).rev() {
        if let Ok(Response::ChannelInfo { name, .. }) =
            Response::decode(&d.ask(&Command::GetChannel { idx })?)
            && (fixed_str(&name).is_empty() || fixed_str(&name) == CHANNEL.as_bytes())
        {
            slot = Some(idx);
            break;
        }
    }
    if let Some(idx) = slot {
        let mut secret = [0u8; 16];
        secret.copy_from_slice(&chan.0[..16]);
        expect_ok(
            &d.ask(&Command::SetChannel {
                idx,
                name: to_fixed::<32>(CHANNEL.as_bytes()),
                secret,
            })?,
            "SET_CHANNEL",
        )?;
        let data = b"oxymesh grp_data";
        let _ = rf.drain()?;
        expect_ok(
            &d.ask(&Command::SendChannelData {
                idx,
                path: Some((0, &[])),
                data_type: 0xFF00,
                data,
            })?,
            "SEND_CHANNEL_DATA",
        )?;
        rep.check(
            "datagramme de canal du C++ déchiffré par mc-air",
            heard(&mut rf, 10, |p| {
                match Payload::parse(p.header.ptype, p.payload) {
                    Ok(Payload::Group {
                        ptype: PayloadType::GRP_DATA,
                        channel,
                        sealed,
                    }) if channel == chan.hash() => {
                        let mut buf = [0u8; 200];
                        let plain = crypto::open(&chan.0, sealed, &mut buf).ok()?;
                        let g = GroupDataPlain::parse(plain).ok()?;
                        Some(if g.data_type == 0xFF00 && g.data == data {
                            Ok(String::new())
                        } else {
                            Err(format!("type {:04x} {}", g.data_type, Hex(g.data)))
                        })
                    }
                    _ => None,
                }
            })
            .and_then(|x| x),
        );

        let msg = "salut le canal, ça va ?";
        let ts_ch = now_unix().max(now + 2);
        let mut plain = [0u8; 200];
        let n = payload::group_txt_plain(ts_ch, ALICE_NAME.as_bytes(), msg.as_bytes(), &mut plain)
            .map_err(|e| format!("{e:?}"))?;
        let raw = zero_hop(
            payload::group_datagram(PayloadType::GRP_TXT, &chan, &plain[..n])
                .map_err(|e| format!("{e:?}"))?,
        )?;
        inject(&mut rf, &raw)?;
        let want = format!("{ALICE_NAME}: {msg}");
        rep.check(
            "message de canal de mc-air lu par le C++",
            fetch(&mut d, |r| match r {
                Response::ChannelMsg {
                    idx: i,
                    text,
                    timestamp,
                    ..
                } if *i == idx => Some(if *text == want.as_bytes() && *timestamp == ts_ch {
                    Ok(String::new())
                } else {
                    Err(format!("« {} »", String::from_utf8_lossy(text)))
                }),
                _ => None,
            })
            .and_then(|x| x),
        );
        // Rend le créneau.
        let _ = d.ask(&Command::SetChannel {
            idx,
            name: [0; 32],
            secret: [0; 16],
        });
    } else {
        rep.check("canal de test", Err("aucun créneau libre".into()));
    }

    let _ = d.ask(&Command::RemoveContact { key: *a.pub_key() });
    println!("\n{} réussis, {} échoués", rep.ok, rep.failed);
    if rep.failed == 0 {
        Ok(())
    } else {
        Err(format!("{} vérifications échouées", rep.failed))
    }
}

fn adv_check(adv: &Advert<'_>, want_name: &[u8]) -> Result<String, String> {
    if !adv.verify() {
        return Err("signature refusée par mc-air".into());
    }
    let ad = AppData::decode(adv.app_data).map_err(|e| format!("données d'annonce : {e:?}"))?;
    if ad.name_str() != want_name {
        return Err(format!(
            "nom « {} »",
            String::from_utf8_lossy(ad.name_str())
        ));
    }
    Ok(format!("signature ok, type {}, nom identique", ad.kind))
}
