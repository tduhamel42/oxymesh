//! `mc duo`: two companions talk to each other over the radio, driven over USB the way an app
//! drives them. Nothing is injected: each device builds its own packets.
//!
//! By default everything stays zero-hop, so no repeater forwards anything:
//! - adverts (`SEND_SELF_ADVERT` without flood);
//! - direct messages over an empty path, with `SENT`, `MSG_WAITING`, `SYNC_NEXT_MESSAGE` and
//!   `SEND_CONFIRMED`;
//! - channel datagrams with a zero-hop path;
//! - telemetry requests as pings (round trip measured here).
//!
//! `--flood` adds the checks that need a flood: channel text both ways, and a flood message
//! whose PATH return teaches the sender a route back.
//!
//! Contacts, the channel slot and telemetry permissions are put back as they were at the end.

use std::time::{Duration, Instant};

use mc_air::crypto::ChannelSecret;
use mc_core::Hex;
use mc_proto::codec::{APP_VER, fixed_str, to_fixed};
use mc_proto::{Command, Response};

use crate::airtest::{Report, expect_ok};
use crate::port::{Port, now_unix};

const CHANNEL: &str = "#oxymesh-test";

/// The contact fields that `ADD_UPDATE_CONTACT` takes, owned.
#[derive(Debug, Clone)]
struct SavedContact {
    kind: u8,
    flags: u8,
    path_len: u8,
    path: [u8; 64],
    name: [u8; 32],
    last_advert: u32,
    lat: i32,
    lon: i32,
}

struct Peer {
    label: &'static str,
    port: Port,
    key: [u8; 32],
    name: String,
    /// `SET_OTHER_PARAMS` values from `SELF_INFO`: manual add, telemetry, location policy,
    /// multi acks.
    other: (u8, u8, u8, u8),
    /// The other device as a contact before the test (`None`: unknown).
    before: Option<SavedContact>,
    /// Test channel slot, and whether it was empty before.
    slot: Option<(u8, bool)>,
}

impl Peer {
    fn prefix(&self) -> [u8; 6] {
        let mut p = [0u8; 6];
        p.copy_from_slice(&self.key[..6]);
        p
    }
}

fn open(path: &str, label: &'static str, verbose: bool) -> Result<Peer, String> {
    let mut port = Port::open(path, label)?;
    port.verbose = verbose;
    let mut info = None;
    for _ in 0..3 {
        if let Ok(f) = port.ask(&Command::DeviceQuery { app_ver: APP_VER }) {
            info = Some(f);
            break;
        }
    }
    let info = info.ok_or_else(|| format!("{label} ({path}) does not respond"))?;
    let firmware = match Response::decode(&info) {
        Ok(Response::DeviceInfo(di)) => format!(
            "{} {}",
            String::from_utf8_lossy(fixed_str(&di.manufacturer)),
            String::from_utf8_lossy(fixed_str(&di.version))
        ),
        _ => "unknown firmware".into(),
    };
    let f = port.ask(&Command::AppStart {
        reserved: [0; 7],
        app_name: b"oxymesh",
    })?;
    let (key, name, other) = match Response::decode(&f) {
        Ok(Response::SelfInfo(si)) => (
            si.key,
            String::from_utf8_lossy(si.node_name).into_owned(),
            (
                si.manual_add,
                si.telemetry,
                si.advert_loc_policy,
                si.multi_acks,
            ),
        ),
        other => return Err(format!("{label}: expected SELF_INFO: {other:?}")),
    };
    let _ = port.ask(&Command::SetDeviceTime { unix_s: now_unix() });
    println!(
        "{label}: {} \"{name}\" — {firmware} ({path})",
        Hex(&key[..4])
    );
    Ok(Peer {
        label,
        port,
        key,
        name,
        other,
        before: None,
        slot: None,
    })
}

fn get_contact(p: &mut Port, key: &[u8; 32]) -> Result<Option<SavedContact>, String> {
    let f = p.ask(&Command::GetContactByKey { key: *key })?;
    Ok(match Response::decode(&f) {
        Ok(Response::Contact(c)) => Some(SavedContact {
            kind: c.kind,
            flags: c.flags,
            path_len: c.path_len,
            path: c.path,
            name: c.name,
            last_advert: c.last_advert,
            lat: c.lat,
            lon: c.lon,
        }),
        _ => None,
    })
}

fn put_contact(p: &mut Port, key: &[u8; 32], c: &SavedContact, path_len: u8) -> Result<(), String> {
    expect_ok(
        &p.ask(&Command::AddUpdateContact {
            key: *key,
            kind: c.kind,
            flags: c.flags,
            path_len,
            path: c.path,
            name: c.name,
            last_advert: c.last_advert,
            lat_lon: Some((c.lat, c.lon)),
            lastmod: None,
        })?,
        "ADD_UPDATE_CONTACT",
    )
}

fn snr(snr4: i8) -> String {
    format!("{:.2} dB", f32::from(snr4) / 4.0)
}

/// Fetches the device's queued messages until `pred` recognizes one, for at most `secs`.
/// Other messages (real traffic) are counted, not printed.
fn fetch<T>(
    p: &mut Port,
    secs: u64,
    mut pred: impl FnMut(&Response<'_>) -> Option<T>,
) -> Result<T, String> {
    let end = Instant::now() + Duration::from_secs(secs);
    let mut skipped = 0;
    loop {
        for _ in 0..300 {
            let f = p.ask(&Command::SyncNextMessage)?;
            let r = Response::decode(&f).map_err(|e| format!("{e:?}"))?;
            if matches!(r, Response::NoMoreMessages) {
                break;
            }
            if let Some(v) = pred(&r) {
                return Ok(v);
            }
            skipped += 1;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!(
                "not received within {secs} s ({skipped} other messages skipped; radio heard: {})",
                heard_summary(p)
            ));
        }
        let _ = p.wait(left.as_secs().max(1), |r| {
            matches!(r, Response::MsgWaiting).then_some(())
        });
    }
}

/// The packets the device reported hearing (`LOG_RX_DATA`) and not consumed yet.
fn heard_summary(p: &mut Port) -> String {
    let frames = p.drain().unwrap_or_default();
    let mut out = Vec::new();
    for f in &frames {
        if let Ok(Response::LogRxData { snr4, raw, .. }) = Response::decode(f) {
            out.push(match mc_air::Packet::decode(raw) {
                Ok(pkt) => format!(
                    "{:?} {:?} path_len {:#04x} SNR {}",
                    pkt.header.ptype,
                    pkt.header.route,
                    pkt.path_len.byte(),
                    snr(snr4)
                ),
                Err(e) => format!("undecodable ({e:?}) {}", Hex(raw)),
            });
        }
    }
    if out.is_empty() {
        "nothing".into()
    } else {
        out.join(", ")
    }
}

/// `from` sends a zero-hop advert; `to` must report it.
fn advert(from: &mut Peer, to: &mut Peer) -> Result<String, String> {
    let _ = to.port.drain()?;
    expect_ok(
        &from
            .port
            .ask(&Command::SendSelfAdvert { flood: Some(false) })?,
        "SEND_SELF_ADVERT",
    )?;
    let key = from.key;
    let t0 = Instant::now();
    let (how, seen) = to.port.wait(10, |r| match r {
        Response::Advert { key: k } if *k == key => Some(("ADVERT", None)),
        Response::NewAdvert(c) if c.key == key => Some((
            "NEW_ADVERT (manual add: added by the test)",
            Some(SavedContact {
                kind: c.kind,
                flags: c.flags,
                path_len: c.path_len,
                path: c.path,
                name: c.name,
                last_advert: c.last_advert,
                lat: c.lat,
                lon: c.lon,
            }),
        )),
        _ => None,
    })?;
    if let Some(c) = seen {
        put_contact(&mut to.port, &key, &c, c.path_len)?;
    }
    Ok(format!("{how} after {} ms", t0.elapsed().as_millis()))
}

/// Direct message `from` → `to`, followed through every companion frame.
fn dm(from: &mut Peer, to: &mut Peer, rep: &mut Report, flood: bool) -> Result<(), String> {
    let label = format!("{} → {}", from.label, to.label);
    let ts = now_unix();
    let text = format!("oxymesh duo {ts} — {} to {}", from.name, to.name);
    let t0 = Instant::now();
    let sent = from.port.ask(&Command::SendTxtMsg {
        txt_type: 0,
        attempt: 0,
        timestamp: ts,
        prefix: to.prefix(),
        text: text.as_bytes(),
    })?;
    let ack = match Response::decode(&sent) {
        Ok(Response::Sent {
            flood: f,
            ack,
            timeout_ms,
        }) => {
            rep.check(
                &format!(
                    "{label}: DM sent {} (SENT)",
                    if flood { "flood" } else { "direct" }
                ),
                if f == flood {
                    Ok(format!("ack {}, timeout {timeout_ms} ms", Hex(&ack)))
                } else {
                    Err(format!("flood = {f}"))
                },
            );
            ack
        }
        other => {
            rep.check(
                &format!("{label}: DM sent (SENT)"),
                Err(format!("{other:?}")),
            );
            return Ok(());
        }
    };
    let prefix = from.prefix();
    rep.check(
        &format!("{label}: DM received (MSG_WAITING → SYNC_NEXT_MESSAGE)"),
        fetch(&mut to.port, 15, |r| match r {
            Response::ContactMsg {
                prefix: p,
                text: t,
                timestamp,
                path_len,
                snr4,
                ..
            } if *p == prefix && *timestamp == ts => Some(if *t == text.as_bytes() {
                Ok(format!(
                    "identical text, path_len {path_len:#04x}, SNR {}",
                    snr(*snr4)
                ))
            } else {
                Err(format!("\"{}\"", String::from_utf8_lossy(t)))
            }),
            _ => None,
        })
        .and_then(|x| x),
    );
    rep.check(
        &format!("{label}: delivery confirmed (SEND_CONFIRMED)"),
        from.port
            .wait(15, |r| match r {
                Response::SendConfirmed { ack: a, rtt_ms } if *a == ack => Some(*rtt_ms),
                _ => None,
            })
            .map(|rtt| {
                format!(
                    "device round trip {rtt} ms, {} ms seen from here",
                    t0.elapsed().as_millis()
                )
            }),
    );
    Ok(())
}

/// A free slot, or the one already holding the test channel; `true` if it was empty.
fn find_slot(p: &mut Port) -> Result<Option<(u8, bool)>, String> {
    for idx in 0..40u8 {
        if let Ok(Response::ChannelInfo { name, .. }) =
            Response::decode(&p.ask(&Command::GetChannel { idx })?)
        {
            let n = fixed_str(&name);
            if n == CHANNEL.as_bytes() {
                return Ok(Some((idx, false)));
            }
            if n.is_empty() {
                return Ok(Some((idx, true)));
            }
        } else {
            break;
        }
    }
    Ok(None)
}

fn channel_data(from: &mut Peer, to: &mut Peer, fi: u8, ti: u8) -> Result<String, String> {
    // GRP_DATA has no nonce: identical bytes in both directions would be dropped as already seen.
    let data = format!("oxymesh duo data {} from {}", now_unix(), from.label).into_bytes();
    expect_ok(
        &from.port.ask(&Command::SendChannelData {
            idx: fi,
            path: Some((0, &[])),
            data_type: 0xFF00,
            data: &data,
        })?,
        "SEND_CHANNEL_DATA",
    )?;
    fetch(&mut to.port, 15, |r| match r {
        Response::ChannelData {
            idx,
            data_type,
            data: d,
            snr4,
            path_len,
            ..
        } if *idx == ti && *d == data.as_slice() => Some(if *data_type == 0xFF00 {
            Ok(format!("path_len {path_len:#04x}, SNR {}", snr(*snr4)))
        } else {
            Err(format!("type {data_type:04x}"))
        }),
        _ => None,
    })
    .and_then(|x| x)
}

fn channel_text(from: &mut Peer, to: &mut Peer, fi: u8, ti: u8) -> Result<String, String> {
    let ts = now_unix();
    let msg = format!("oxymesh duo {ts} — channel test");
    expect_ok(
        &from.port.ask(&Command::SendChannelTxtMsg {
            txt_type: 0,
            idx: fi,
            timestamp: ts,
            text: msg.as_bytes(),
        })?,
        "SEND_CHANNEL_TXT_MSG",
    )?;
    let want = format!("{}: {msg}", from.name);
    fetch(&mut to.port, 20, |r| match r {
        Response::ChannelMsg {
            idx,
            text,
            timestamp,
            path_len,
            ..
        } if *idx == ti && *timestamp == ts => Some(if *text == want.as_bytes() {
            Ok(format!("identical text, path_len {path_len:#04x}"))
        } else {
            Err(format!("\"{}\"", String::from_utf8_lossy(text)))
        }),
        _ => None,
    })
    .and_then(|x| x)
}

/// Packet counters and core state (`GET_STATS` 2 and 0), compact, for diagnostics.
fn stats(p: &mut Port) -> String {
    let mut out = String::new();
    if let Ok(f) = p.ask(&Command::GetStats { kind: 2 })
        && let Ok(Response::Stats(mc_proto::response::Stats::Packets {
            recv,
            sent,
            recv_errors,
            ..
        })) = Response::decode(&f)
    {
        out.push_str(&format!("rx {recv} tx {sent} rx_err {recv_errors}"));
    }
    if let Ok(f) = p.ask(&Command::GetStats { kind: 0 })
        && let Ok(Response::Stats(mc_proto::response::Stats::Core {
            err_flags,
            queue_len,
            ..
        })) = Response::decode(&f)
    {
        out.push_str(&format!(" err_flags {err_flags:#06x} queue {queue_len}"));
    }
    out
}

/// Telemetry requests from `from` to `to` used as pings; `to` temporarily allows base
/// telemetry for everyone.
fn pings(from: &mut Peer, to: &mut Peer, n: u32) -> Result<String, String> {
    let (manual, tele, loc, multi) = to.other;
    let set = |t: u8| Command::SetOtherParams {
        manual_add: manual,
        telemetry: Some(t),
        advert_loc_policy: Some(loc),
        multi_acks: Some(multi),
    };
    expect_ok(&to.port.ask(&set((tele & !3) | 2))?, "SET_OTHER_PARAMS")?;
    // Pings sent within about a second of a prefs change lose their second exchange with the
    // C++ firmware (docs/measurements.md, 2026-11-06); let the devices settle first.
    std::thread::sleep(Duration::from_secs(3));
    let mut rtts = Vec::new();
    let mut failures = Vec::new();
    let prefix = to.prefix();
    // Counters before the first ping only: querying between pings changes the timing.
    let before = (stats(&mut from.port), stats(&mut to.port));
    for _ in 0..n {
        let t0 = Instant::now();
        let sent = from.port.ask(&Command::SendTelemetryReq {
            reserved: [0; 3],
            key: Some(to.key),
        })?;
        if !matches!(Response::decode(&sent), Ok(Response::Sent { .. })) {
            failures.push(format!("{:?}", Response::decode(&sent)));
            continue;
        }
        match from.port.wait(10, |r| match r {
            Response::TelemetryResponse { prefix: p, lpp, .. } if *p == prefix => Some(lpp.len()),
            _ => None,
        }) {
            Ok(_) => rtts.push(t0.elapsed().as_millis()),
            Err(_) => failures.push(format!(
                "ping {} unanswered after 10 s; {} heard: {}; {} heard: {}; stats {} at start [{}] now [{}], {} at start [{}] now [{}]",
                rtts.len() + failures.len() + 1,
                to.label,
                heard_summary(&mut to.port),
                from.label,
                heard_summary(&mut from.port),
                from.label,
                before.0,
                stats(&mut from.port),
                to.label,
                before.1,
                stats(&mut to.port),
            )),
        }
    }
    let restored = to.port.ask(&set(tele));
    expect_ok(&restored?, "SET_OTHER_PARAMS (restore)")?;
    if rtts.is_empty() {
        return Err(format!("0/{n} answered: {}", failures.join("; ")));
    }
    let min = rtts.iter().min().copied().unwrap_or(0);
    let max = rtts.iter().max().copied().unwrap_or(0);
    let avg = rtts.iter().sum::<u128>() / rtts.len() as u128;
    let line = format!(
        "{}/{n} answered, RTT min {min} / avg {avg} / max {max} ms",
        rtts.len()
    );
    if failures.is_empty() {
        Ok(line)
    } else {
        Err(format!("{line}; lost: {}", failures.join("; ")))
    }
}

/// B writes to A, A reboots before the app fetches it, and must still deliver it: the offline
/// queue lives in flash (deliberate deviation 1).
fn reboot_keeps(a: &mut Peer, b: &mut Peer) -> Result<String, String> {
    let ts = now_unix();
    let text = format!("oxymesh duo {ts} — kept across a reboot");
    let sent = b.port.ask(&Command::SendTxtMsg {
        txt_type: 0,
        attempt: 0,
        timestamp: ts,
        prefix: a.prefix(),
        text: text.as_bytes(),
    })?;
    if !matches!(Response::decode(&sent), Ok(Response::Sent { .. })) {
        return Err(format!("SEND_TXT_MSG: {:?}", Response::decode(&sent)));
    }
    a.port
        .wait(15, |r| matches!(r, Response::MsgWaiting).then_some(()))?;
    std::thread::sleep(Duration::from_millis(500));
    a.port.send(&Command::Reboot)?;
    std::thread::sleep(Duration::from_secs(3));
    let t0 = Instant::now();
    // The USB bridge stays open through the ESP32's reset: same port, new session.
    let _ = a.port.drain();
    let mut back = false;
    for _ in 0..5 {
        if a.port
            .ask(&Command::DeviceQuery { app_ver: APP_VER })
            .is_ok()
        {
            back = true;
            break;
        }
    }
    if !back {
        return Err("A did not come back after the reboot".into());
    }
    let _ = a.port.ask(&Command::AppStart {
        reserved: [0; 7],
        app_name: b"oxymesh",
    });
    let prefix = b.prefix();
    fetch(&mut a.port, 5, |r| match r {
        Response::ContactMsg {
            prefix: p,
            text: t,
            timestamp,
            ..
        } if *p == prefix && *timestamp == ts => Some(if *t == text.as_bytes() {
            Ok(format!("back after {} ms", t0.elapsed().as_millis()))
        } else {
            Err(format!("\"{}\"", String::from_utf8_lossy(t)))
        }),
        _ => None,
    })
    .and_then(|x| x)
}

fn checks(
    a: &mut Peer,
    b: &mut Peer,
    rep: &mut Report,
    flood: bool,
    n_pings: u32,
    reboot: bool,
) -> Result<(), String> {
    // 1. Zero-hop adverts both ways.
    rep.check("A advert heard by B", advert(a, b));
    rep.check("B advert heard by A", advert(b, a));

    // Each side must now know the other; pin the route to zero hop.
    let (ak, bk) = (a.key, b.key);
    for (p, other) in [(&mut *a, bk), (&mut *b, ak)] {
        let c = get_contact(&mut p.port, &other)?
            .ok_or_else(|| format!("{}: the other device is not a contact", p.label))?;
        put_contact(&mut p.port, &other, &c, 0)?;
    }

    // 2. Direct messages both ways.
    dm(a, b, rep, false)?;
    dm(b, a, rep, false)?;

    // 3. Channel datagrams, zero hop.
    a.slot = find_slot(&mut a.port)?;
    b.slot = find_slot(&mut b.port)?;
    if let (Some((ia, _)), Some((ib, _))) = (a.slot, b.slot) {
        let chan = ChannelSecret::from_hashtag(CHANNEL);
        let mut secret = [0u8; 16];
        secret.copy_from_slice(&chan.0[..16]);
        for (p, idx) in [(&mut *a, ia), (&mut *b, ib)] {
            expect_ok(
                &p.port.ask(&Command::SetChannel {
                    idx,
                    name: to_fixed::<32>(CHANNEL.as_bytes()),
                    secret,
                })?,
                "SET_CHANNEL",
            )?;
        }
        rep.check(
            "A → B: channel datagram, zero hop",
            channel_data(a, b, ia, ib),
        );
        rep.check(
            "B → A: channel datagram, zero hop",
            channel_data(b, a, ib, ia),
        );
        if flood {
            rep.check("A → B: channel text (flood)", channel_text(a, b, ia, ib));
            rep.check("B → A: channel text (flood)", channel_text(b, a, ib, ia));
        }
    } else {
        rep.check("test channel", Err("no free channel slot".into()));
    }

    // 4. Telemetry pings both ways.
    if n_pings > 0 {
        rep.check("A → B: telemetry pings", pings(a, b, n_pings));
        rep.check("B → A: telemetry pings", pings(b, a, n_pings));
    }

    // 5. Flood message and the path it teaches back.
    if flood {
        let bk = b.key;
        expect_ok(&a.port.ask(&Command::ResetPath { key: bk })?, "RESET_PATH")?;
        let _ = a.port.drain()?;
        dm(a, b, rep, true)?;
        rep.check(
            "A learns its route to B from the PATH return",
            a.port
                .wait(10, |r| match r {
                    Response::PathUpdated { key } if *key == bk => Some(()),
                    _ => None,
                })
                .and_then(|()| {
                    let c = get_contact(&mut a.port, &bk)?.ok_or("B is no longer a contact")?;
                    Ok(format!("PATH_UPDATED, path_len {:#04x}", c.path_len))
                }),
        );
    } else {
        println!("  (channel text and flood routing skipped: they need --flood)");
    }

    // 6. The offline queue survives a reboot (only on an oxymesh A: it rebooted the device).
    if reboot {
        rep.check(
            "A keeps an unfetched DM across a reboot",
            reboot_keeps(a, b),
        );
    }
    Ok(())
}

/// Puts contacts and the channel slot back as they were.
fn restore(a: &mut Peer, b: &mut Peer) {
    let (ak, bk) = (a.key, b.key);
    for (p, other) in [(&mut *a, bk), (&mut *b, ak)] {
        let r = match &p.before {
            Some(c) => put_contact(&mut p.port, &other, c, c.path_len),
            None => p
                .port
                .ask(&Command::RemoveContact { key: other })
                .map(|_| ()),
        };
        if let Some((idx, true)) = p.slot {
            let _ = p.port.ask(&Command::SetChannel {
                idx,
                name: [0; 32],
                secret: [0; 16],
            });
        }
        if let Err(e) = r {
            println!("  {}: could not restore the contact: {e}", p.label);
        }
    }
}

pub fn run(
    a_path: &str,
    b_path: &str,
    flood: bool,
    n_pings: u32,
    reboot: bool,
    verbose: bool,
) -> Result<(), String> {
    let mut a = open(a_path, "A", verbose)?;
    let mut b = open(b_path, "B", verbose)?;
    if a.key == b.key {
        return Err("both ports reach the same device".into());
    }
    a.before = get_contact(&mut a.port, &b.key)?;
    b.before = get_contact(&mut b.port, &a.key)?;
    println!();
    let mut rep = Report { ok: 0, failed: 0 };
    let res = checks(&mut a, &mut b, &mut rep, flood, n_pings, reboot);
    restore(&mut a, &mut b);
    res?;
    println!("\n{} passed, {} failed", rep.ok, rep.failed);
    if rep.failed == 0 {
        Ok(())
    } else {
        Err(format!("{} checks failed", rep.failed))
    }
}
