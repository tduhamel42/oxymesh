//! `mc`: tools around a MeshCore companion connected over USB.

mod airtest;
mod describe;
mod duo;
mod png;
mod port;
mod ui;

use clap::{Parser, Subcommand};
use mc_air::crypto::ChannelSecret;
use mc_proto::codec::APP_VER;
use mc_proto::{Command, Response};

use crate::describe::{Keys, describe};
use crate::port::Port;

#[derive(Parser)]
#[command(name = "mc", about = "oxymesh tools for MeshCore companions")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Query the device: version, settings, scope.
    Info { port: String },
    /// Print, decoded, the frames the device hears (LOG_RX_DATA).
    Sniff {
        port: String,
        /// Duration in seconds (endless by default).
        #[arg(long)]
        secs: Option<u64>,
        /// Channel to decrypt in addition to the Public channel: `#name` or `name=hex secret (16 bytes)`.
        #[arg(long)]
        channel: Vec<String>,
    },
    /// Send a direct message to a contact, by the hex prefix of its key (6 bytes or more).
    Send {
        port: String,
        to: String,
        text: String,
    },
    /// Fetch and print the messages waiting on the device (SYNC_NEXT_MESSAGE until empty).
    Inbox { port: String },
    /// Make the device send its advert (zero hop unless `--flood`).
    Advert {
        port: String,
        #[arg(long)]
        flood: bool,
    },
    /// Make the device transmit a raw frame (SEND_RAW_PACKET).
    Inject {
        port: String,
        hex: String,
        #[arg(long, default_value_t = 0)]
        prio: u8,
    },
    /// Test mc-air against a C++ firmware: `--ref` transmits and listens, `--dut` is under test.
    Airtest {
        #[arg(long = "ref")]
        ref_port: String,
        #[arg(long)]
        dut: String,
        #[arg(short, long)]
        verbose: bool,
    },
    /// The device's interface in a terminal, on a simulated device (`--png DIR`: panel images).
    Ui {
        /// Compact text (33×20) instead of large (25×15).
        #[arg(long)]
        compact: bool,
        /// Render every screen of the tour as PNG files in this directory, then exit.
        #[arg(long)]
        png: Option<std::path::PathBuf>,
    },
    /// Two companions talk to each other (adverts, DMs, channel, pings), zero-hop unless `--flood`.
    Duo {
        #[arg(long)]
        a: String,
        #[arg(long)]
        b: String,
        /// Also run the checks that flood: channel text and route learning.
        #[arg(long)]
        flood: bool,
        /// Telemetry pings in each direction.
        #[arg(long, default_value_t = 3)]
        pings: u32,
        /// Also reboot A with a DM waiting and check it is still there (oxymesh on A).
        #[arg(long)]
        reboot: bool,
        #[arg(short, long)]
        verbose: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Info { port } => info(&port),
        Cmd::Sniff {
            port,
            secs,
            channel,
        } => sniff(&port, secs, &channel),
        Cmd::Inject { port, hex, prio } => inject(&port, &hex, prio),
        Cmd::Advert { port, flood } => advert(&port, flood),
        Cmd::Inbox { port } => inbox(&port),
        Cmd::Send { port, to, text } => send(&port, &to, &text),
        Cmd::Airtest {
            ref_port,
            dut,
            verbose,
        } => airtest::run(&ref_port, &dut, verbose),
        Cmd::Ui { compact, png } => ui::run(compact, png.as_deref()),
        Cmd::Duo {
            a,
            b,
            flood,
            pings,
            reboot,
            verbose,
        } => duo::run(&a, &b, flood, pings, reboot, verbose),
    };
    if let Err(e) = res {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn info(path: &str) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    p.ask(&Command::AppStart {
        reserved: [0; 7],
        app_name: b"oxymesh",
    })?;
    p.ask(&Command::GetDefaultFloodScope)?;
    p.ask(&Command::GetDeviceTime)?;
    Ok(())
}

fn sniff(path: &str, secs: Option<u64>, channels: &[String]) -> Result<(), String> {
    let mut keys = Keys::default();
    keys.channels.push(("Public".into(), ChannelSecret::PUBLIC));
    for c in channels {
        if let Some((name, hex)) = c.split_once('=') {
            let mut k = [0u8; 16];
            let n = mc_core::hex::decode_into(hex, &mut k).ok_or("invalid hex secret")?;
            if n != 16 {
                return Err("the secret must be 16 bytes".into());
            }
            keys.channels
                .push((name.into(), ChannelSecret::from_psk16(&k)));
        } else {
            keys.channels
                .push((c.clone(), ChannelSecret::from_hashtag(c)));
        }
    }
    let mut p = Port::open(path, "device")?;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    let end = secs.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
    while end.is_none_or(|e| std::time::Instant::now() < e) {
        for f in p.drain()? {
            if let Ok(Response::LogRxData { snr4, rssi, raw }) = Response::decode(&f) {
                println!(
                    "snr={:>5.2} rssi={rssi:>4} {}",
                    f32::from(snr4) / 4.0,
                    describe(raw, &keys)
                );
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}

fn send(path: &str, to: &str, text: &str) -> Result<(), String> {
    let mut prefix = [0u8; 32];
    let n = mc_core::hex::decode_into(to, &mut prefix).ok_or("invalid hex prefix")?;
    if n < 6 {
        return Err("the prefix needs at least 6 bytes (12 hex digits)".into());
    }
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    let now = crate::port::now_unix();
    let sent = p.ask(&Command::SendTxtMsg {
        txt_type: 0,
        attempt: 0,
        timestamp: now,
        prefix: prefix[..6].try_into().unwrap_or([0; 6]),
        text: text.as_bytes(),
    })?;
    if let Ok(Response::Sent { ack, .. }) = Response::decode(&sent) {
        let got = p.wait(20, |r| match r {
            Response::SendConfirmed { ack: a, rtt_ms } if *a == ack => Some(*rtt_ms),
            _ => None,
        });
        match got {
            Ok(rtt) => println!("delivered, round trip {rtt} ms"),
            Err(_) => println!("no delivery confirmation within 20 s"),
        }
    }
    Ok(())
}

fn inbox(path: &str) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    let mut n = 0;
    loop {
        let f = p.ask(&Command::SyncNextMessage)?;
        match Response::decode(&f) {
            Ok(Response::NoMoreMessages) => break,
            Ok(Response::ContactMsg {
                prefix,
                text,
                timestamp,
                path_len,
                snr4,
                ..
            }) => println!(
                "DM from {} at {timestamp} path_len {path_len:#04x} SNR {:.2}: {}",
                mc_core::Hex(&prefix),
                f32::from(snr4) / 4.0,
                String::from_utf8_lossy(text)
            ),
            Ok(Response::ChannelMsg { idx, text, .. }) => {
                println!("channel {idx}: {}", String::from_utf8_lossy(text))
            }
            Ok(other) => println!("{other}"),
            Err(e) => println!("unreadable frame ({e:?})"),
        }
        n += 1;
    }
    println!("{n} message(s)");
    Ok(())
}

fn advert(path: &str, flood: bool) -> Result<(), String> {
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::DeviceQuery { app_ver: APP_VER })?;
    p.ask(&Command::SetDeviceTime {
        unix_s: crate::port::now_unix(),
    })?;
    p.ask(&Command::SendSelfAdvert { flood: Some(flood) })?;
    Ok(())
}

fn inject(path: &str, hex: &str, prio: u8) -> Result<(), String> {
    let mut raw = [0u8; 255];
    let n = mc_core::hex::decode_into(hex, &mut raw).ok_or("invalid hex")?;
    mc_air::Packet::decode(&raw[..n]).map_err(|e| format!("invalid frame: {e:?}"))?;
    let mut p = Port::open(path, "device")?;
    p.verbose = true;
    p.ask(&Command::SendRawPacket {
        priority: prio,
        raw: &raw[..n],
    })?;
    Ok(())
}
