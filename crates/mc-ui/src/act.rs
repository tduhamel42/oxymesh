//! What the screens ask for, and the one place that applies it to the device. The firmware and
//! the emulator both call [`perform`]; only the light, reboot and factory reset come back to
//! them as outcomes.

use alloc::format;
use alloc::string::String;

use mc_core::Instant;
use mc_node::device::ReqKind;
use mc_node::history::Peer;
use mc_node::{Device, Storage};
use rand_core::Rng;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    ToggleLight,
    Reboot,
    FactoryReset,
    SendAdvert {
        flood: bool,
    },
    SendText {
        key: [u8; 32],
        text: String,
    },
    SendChannel {
        idx: u8,
        text: String,
    },
    MarkRead(Peer),
    /// Telemetry request used as a ping.
    Ping([u8; 32]),
    /// Trace along repeater hashes (`flags & 3`: hash size code).
    Trace {
        flags: u8,
        path: alloc::vec::Vec<u8>,
    },
    /// Zero-hop discovery of the repeaters around.
    Discover,
    /// Login to a repeater (empty password: its access list only).
    Login {
        key: [u8; 32],
        password: String,
    },
    RepeaterStatus([u8; 32]),
    Neighbours([u8; 32]),
    /// Our position in adverts, degrees × 1e6.
    SetLocation {
        lat: i32,
        lon: i32,
    },
    ResetPath([u8; 32]),
    SetFavourite([u8; 32], bool),
    RemoveContact([u8; 32]),
    /// A `#hashtag` channel, name without the `#`.
    AddChannel(String),
    RemoveChannel(u8),
    SetName(String),
    SetRadio {
        freq_khz: u32,
        bw_hz: u32,
        sf: u8,
        cr: u8,
    },
    SetupDone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// Companion error code, or a send failure.
    Failed(u8),
    /// A history entry was created (messages).
    Message(u32),
    /// A request is in flight: its tag (`Device::request_reply`).
    Request(u32),
    /// A channel slot (added channel).
    Channel(u8),
    /// For the runner: toggle the backlight.
    Light,
    /// For the runner: save, then reboot.
    Reboot,
    /// For the runner: erase the storage, then reboot.
    FactoryReset,
}

fn of(r: Result<(), u8>) -> Outcome {
    match r {
        Ok(()) => Outcome::Ok,
        Err(e) => Outcome::Failed(e),
    }
}

fn send_err(e: mc_node::SendError) -> Outcome {
    Outcome::Failed(match e {
        mc_node::SendError::NotFound => 2,
        mc_node::SendError::Failed => 3,
        mc_node::SendError::Unsupported => 1,
    })
}

/// Applies an intent to the device.
pub fn perform(
    i: &Intent,
    dev: &mut Device,
    now: Instant,
    rng: &mut impl Rng,
    st: &mut impl Storage,
) -> Outcome {
    match i {
        Intent::ToggleLight => Outcome::Light,
        Intent::Reboot => {
            dev.flush_contacts(st);
            Outcome::Reboot
        }
        Intent::FactoryReset => Outcome::FactoryReset,
        Intent::SendAdvert { flood } => match dev.node.send_self_advert(now, *flood) {
            Ok(()) => Outcome::Ok,
            Err(e) => send_err(e),
        },
        Intent::SendText { key, text } => match dev.send_text(now, key, text.as_bytes(), st) {
            Ok(seq) => Outcome::Message(seq),
            Err(e) => send_err(e),
        },
        Intent::SendChannel { idx, text } => {
            match dev.send_channel_text(now, *idx, text.as_bytes(), st) {
                Ok(seq) => Outcome::Message(seq),
                Err(e) => send_err(e),
            }
        }
        Intent::MarkRead(peer) => {
            dev.mark_read(*peer, st);
            Outcome::Ok
        }
        Intent::Ping(key) => match dev.request(now, key, ReqKind::Telemetry, rng) {
            Ok(tag) => Outcome::Request(tag),
            Err(e) => send_err(e),
        },
        Intent::Trace { flags, path } => match dev.request_trace(now, *flags, path, rng) {
            Ok(tag) => Outcome::Request(tag),
            Err(e) => send_err(e),
        },
        Intent::SetLocation { lat, lon } => of(dev.set_location(*lat, *lon, st)),
        Intent::Discover => match dev.discover(now, rng) {
            Ok(tag) => Outcome::Request(tag),
            Err(e) => send_err(e),
        },
        Intent::Login { key, password } => match dev.request_login(now, key, password.as_bytes()) {
            Ok(tag) => Outcome::Request(tag),
            Err(e) => send_err(e),
        },
        Intent::RepeaterStatus(key) => match dev.request(now, key, ReqKind::Status, rng) {
            Ok(tag) => Outcome::Request(tag),
            Err(e) => send_err(e),
        },
        Intent::Neighbours(key) => match dev.request_neighbours(now, key, rng) {
            Ok(tag) => Outcome::Request(tag),
            Err(e) => send_err(e),
        },
        Intent::ResetPath(key) => of(dev.reset_contact_path(now, key)),
        Intent::SetFavourite(key, on) => of(dev.set_favourite(now, key, *on)),
        Intent::RemoveContact(key) => of(dev.remove_contact(now, key, st)),
        Intent::AddChannel(name) => match dev.add_hashtag_channel(&format!("#{name}"), st) {
            Ok(idx) => Outcome::Channel(idx),
            Err(e) => Outcome::Failed(e),
        },
        Intent::RemoveChannel(idx) => of(dev.remove_channel(*idx, st)),
        Intent::SetName(name) => {
            dev.set_name(name.as_bytes(), st);
            Outcome::Ok
        }
        Intent::SetRadio {
            freq_khz,
            bw_hz,
            sf,
            cr,
        } => of(dev.set_radio(*freq_khz, *bw_hz, *sf, *cr, false, st)),
        Intent::SetupDone => {
            let mut ui = dev.node.prefs.ui;
            ui.setup_done = true;
            dev.set_ui(ui, st);
            Outcome::Ok
        }
    }
}
