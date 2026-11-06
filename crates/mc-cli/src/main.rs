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
