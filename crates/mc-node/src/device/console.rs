//! The device console, as the C++ companion answers it (`MyMesh::handleCommand`,
//! `examples/companion_radio/MyMesh.cpp:2079-2211`, MeshCore, MIT): from the app
//! (`RUN_CLI_COMMAND`) and, for contacts allowed to, over the air (`TXT_TYPE_CLI_COMMAND`).
//! Replies are the C++ texts. `docs/spec/companion.md` §6.

use core::fmt::Write;

use mc_core::Instant;

use super::{Action, Device, FrontEnd, Storage};
use crate::cli::{self, Effect, Reply};
use crate::contacts::name_str;

/// What to do with the reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    Send,
    /// The device restarts or powers off: the C++ never answers.
    Silent,
}

impl Device {
    /// Runs one console command; `cmd` is the text up to its first NUL.
    pub(crate) fn console(
        &mut self,
        cmd: &[u8],
        out: &mut Reply,
        st: &mut impl Storage,
    ) -> Delivery {
        let mut b = name_str(cmd);
        while let [b' ', rest @ ..] = b {
            b = rest;
        }
        // Optional "xx|" prefix (companion radio CLI), reflected back.
        if b.len() > 4
            && b.get(2) == Some(&b'|')
            && let (Some(tag), Some(rest)) = (b.get(..3), b.get(3..))
        {
            for &c in tag {
                let _ = out.push(char::from(c));
            }
            b = rest;
        }
        let Ok(c) = core::str::from_utf8(b) else {
            let _ = out.push_str("Unknown command");
            return Delivery::Send;
        };
        if let Some(effect) = cli::radio_command(&mut self.node.prefs, c, out) {
            if effect != Effect::None {
                self.save_prefs(st);
            }
            let tx_ok = (-9..=self.info.max_tx_power).contains(&self.node.prefs.radio.tx_power_dbm);
            if effect == Effect::RxGain || (effect == Effect::TxPower && tx_ok) {
                let _ = self.actions.push_back(Action::RadioChanged);
            }
            return Delivery::Send;
        }
        if let Some(d) = self.board_command(c, out, st) {
            return d;
        }
        match c {
            "reboot" => {
                let _ = self.actions.push_back(Action::Reboot);
                return Delivery::Silent;
            }
            "poweroff" | "shutdown" => {
                let _ = self.actions.push_back(Action::PowerOff);
                return Delivery::Silent;
            }
            "get name" => {
                let _ = out.push_str("> ");
                push_bytes(out, name_str(&self.node.prefs.name));
            }
            "board" => {
                let _ = out.push_str(self.info.manufacturer);
            }
            "ver" => {
                let _ = write!(
                    out,
                    "{} (Build: {})",
                    self.info.version, self.info.build_date
                );
            }
            "get tz.offset" => {
                let _ = write!(out, "> {}", self.node.prefs.ui.tz_offset_min / 60);
            }
            _ => {
                if let Some(name) = c.strip_prefix("set name ") {
                    if cli::valid_name(name.as_bytes()) {
                        self.set_name(name.as_bytes(), st);
                        let _ = out.push_str("OK");
                    } else {
                        let _ = out.push_str("Error, bad chars");
                    }
                } else if let Some(v) = c.strip_prefix("set pin ") {
                    let pin = cli::atoi(v);
                    self.node.prefs.ble_pin = pin as u32;
                    self.save_prefs(st);
                    let _ = self.actions.push_back(Action::PinChanged);
                    let _ = write!(out, "> pin is now {pin:06}");
                } else if let Some(v) = c.strip_prefix("set tz.offset ") {
                    // `int8_t tz = atof(…)`: truncated toward zero, then range-checked.
                    let tz = (cli::atof_micro(v) / 1_000_000) as i8;
                    if !(-12..=14).contains(&tz) {
                        let _ = out.push_str("Error, must be from -12 to +14");
                    } else {
                        let mut ui = self.node.prefs.ui;
                        ui.tz_offset_min = i16::from(tz) * 60;
                        self.set_ui(ui, st);
                        let _ = out.push_str("OK");
                    }
                } else {
                    let _ = out.push_str("Unknown command");
                }
            }
        }
        Delivery::Send
    }

    /// The board's own commands (`board.handleCommand`): the Heltec V4's front-end LNA
    /// (`variants/heltec_v4/HeltecV4Board.cpp:89-122`).
    fn board_command(
        &mut self,
        c: &str,
        out: &mut Reply,
        st: &mut impl Storage,
    ) -> Option<Delivery> {
        let get = c == "get radio.fem.rxgain";
        let set = c.strip_prefix("set radio.fem.rxgain ");
        if !get && set.is_none() {
            return None;
        }
        match self.info.front_end {
            FrontEnd::None => return None,
            FrontEnd::Pa => {
                let _ = out.push_str("Error: unsupported");
            }
            FrontEnd::PaLna => {
                if get {
                    let _ = out.push_str(if self.node.prefs.fem_rxgain {
                        "> on"
                    } else {
                        "> off"
                    });
                } else if let Some(v) = set {
                    let on = if v.starts_with("on") {
                        true
                    } else if v.starts_with("off") {
                        false
                    } else {
                        let _ = out.push_str("Error: state must be on or off");
                        return Some(Delivery::Send);
                    };
                    self.node.prefs.fem_rxgain = on;
                    self.save_prefs(st);
                    let _ = self.actions.push_back(Action::RadioChanged);
                    let _ = write!(
                        out,
                        "OK - LoRa FEM RX gain {}",
                        if on { "on" } else { "off" }
                    );
                }
            }
        }
        Some(Delivery::Send)
    }

    /// A console command received over the air from a contact (`onCLICommandRecv`,
    /// `MyMesh.cpp:538-548`; `BaseChatMesh.cpp:263-292`): run if the contact may (flag 0x10),
    /// answered as CLI data 600 ms later; otherwise it goes to the app. True if it was run.
    pub(crate) fn remote_console(
        &mut self,
        now: Instant,
        key: &[u8; 32],
        sender_ts: u32,
        text: &[u8],
        st: &mut impl Storage,
    ) -> bool {
        let Some(slot) = self.node.contacts.find(key) else {
            return false;
        };
        let allowed = self
            .node
            .contacts
            .get(slot)
            .is_some_and(|c| c.flags & REMOTE_CLI != 0);
        if !allowed {
            return false;
        }
        let mut reply = Reply::new();
        if self.console(text, &mut reply, st) == Delivery::Send && !reply.is_empty() {
            let _ = self
                .node
                .send_cli_reply(now, slot, sender_ts, reply.as_bytes());
        }
        true
    }
}

/// Contact flag: may run console commands on this device (`isRemoteCLIAllowed`).
pub const REMOTE_CLI: u8 = 0x10;

fn push_bytes(out: &mut Reply, b: &[u8]) {
    match core::str::from_utf8(b) {
        Ok(s) => {
            let _ = out.push_str(s);
        }
        Err(_) => {
            for &c in b {
                let _ = out.push(if c.is_ascii() { char::from(c) } else { '?' });
            }
        }
    }
}
