//! One settings API for the companion protocol, the device CLI and the screen. Each setter
//! checks its arguments the way the C++ command does, saves at once, and asks the runner to
//! reconfigure the radio when needed. Errors are the companion error codes (`err::*`).

use mc_proto::codec::err;

use mc_core::Instant;

use super::{Action, Device, REPEAT_FREQS, Storage};
use crate::contacts::{Channel, MAX_CHANNELS, Slot, name_str};
use crate::node::UiPrefs;
use crate::persist::encode_channel;

/// `Err` carries the companion error code.
pub type SetResult = Result<(), u8>;

impl Device {
    /// `SET_ADVERT_NAME`: cut to 31 bytes, at the first NUL.
    pub fn set_name(&mut self, name: &[u8], st: &mut impl Storage) {
        let mut n = [0u8; 32];
        for (o, b) in n.iter_mut().zip(name_str(name).iter().take(31)) {
            *o = *b;
        }
        self.node.prefs.name = n;
        self.save_prefs(st);
    }

    /// `SET_RADIO_PARAMS` (`MyMesh.cpp:1032-1060`).
    pub fn set_radio(
        &mut self,
        freq_khz: u32,
        bw_hz: u32,
        sf: u8,
        cr: u8,
        repeat: bool,
        st: &mut impl Storage,
    ) -> SetResult {
        let ok = (150_000..=2_500_000).contains(&freq_khz)
            && (7_000..=500_000).contains(&bw_hz)
            && (5..=12).contains(&sf)
            && (5..=8).contains(&cr);
        if !ok || (repeat && !REPEAT_FREQS.contains(&freq_khz)) {
            return Err(err::ILLEGAL_ARG);
        }
        let r = &mut self.node.prefs.radio;
        r.freq_khz = freq_khz;
        r.bw_hz = bw_hz;
        r.sf = sf;
        r.cr = cr;
        self.node.prefs.repeat = repeat;
        self.save_prefs(st);
        let _ = self.actions.push_back(Action::RadioChanged);
        Ok(())
    }

    /// `SET_RADIO_TX_POWER`: −9 dBm to the board's maximum.
    pub fn set_tx_power(&mut self, dbm: i8, st: &mut impl Storage) -> SetResult {
        if dbm < -9 || dbm > self.info.max_tx_power {
            return Err(err::ILLEGAL_ARG);
        }
        self.node.prefs.radio.tx_power_dbm = dbm;
        self.save_prefs(st);
        let _ = self.actions.push_back(Action::RadioChanged);
        Ok(())
    }

    /// `SET_ADVERT_LATLON`, degrees × 1e6.
    pub fn set_location(&mut self, lat: i32, lon: i32, st: &mut impl Storage) -> SetResult {
        if !(-90_000_000..=90_000_000).contains(&lat)
            || !(-180_000_000..=180_000_000).contains(&lon)
        {
            return Err(err::ILLEGAL_ARG);
        }
        self.node.prefs.lat = lat;
        self.node.prefs.lon = lon;
        self.save_prefs(st);
        Ok(())
    }

    /// `SET_TUNING_PARAMS`, values × 1000.
    pub fn set_tuning(&mut self, rx_delay: u32, airtime_factor: u32, st: &mut impl Storage) {
        self.node.prefs.rx_delay_base_milli = rx_delay;
        self.node.prefs.airtime_factor_milli = airtime_factor;
        self.save_prefs(st);
    }

    /// `SET_CHANNEL` with a 16-byte secret.
    pub fn set_channel(
        &mut self,
        idx: u8,
        name: [u8; 32],
        secret: [u8; 16],
        st: &mut impl Storage,
    ) -> SetResult {
        let c = self
            .node
            .channels
            .0
            .get_mut(usize::from(idx))
            .ok_or(err::NOT_FOUND)?;
        *c = Channel {
            name,
            secret: mc_air::ChannelSecret::from_psk16(&secret),
        };
        let rec = encode_channel(c);
        st.save_channel(idx, &rec);
        Ok(())
    }

    /// The screen adds a `#hashtag` channel (secret derived from its name, like the app):
    /// the slot it already has, or the first free one.
    pub fn add_hashtag_channel(&mut self, name: &str, st: &mut impl Storage) -> Result<u8, u8> {
        let mut n = [0u8; 32];
        for (o, b) in n.iter_mut().zip(name.bytes().take(31)) {
            *o = b;
        }
        let channels = &self.node.channels.0;
        let idx = channels
            .iter()
            .position(|c| c.name == n)
            .or_else(|| channels.iter().position(Channel::is_empty))
            .ok_or(err::TABLE_FULL)?;
        let idx = u8::try_from(idx).map_err(|_| err::TABLE_FULL)?;
        let secret = mc_air::ChannelSecret::from_hashtag(name);
        let mut psk = [0u8; 16];
        psk.copy_from_slice(secret.0.get(..16).ok_or(err::ILLEGAL_ARG)?);
        self.set_channel(idx, n, psk, st)?;
        Ok(idx)
    }

    /// Empties a channel slot (`SET_CHANNEL` with an empty name and secret).
    pub fn remove_channel(&mut self, idx: u8, st: &mut impl Storage) -> SetResult {
        if usize::from(idx) >= MAX_CHANNELS {
            return Err(err::NOT_FOUND);
        }
        self.set_channel(idx, [0; 32], [0; 16], st)
    }

    /// `SET_DEVICE_PIN`: 0 (random at pairing) or six digits; applied at reboot.
    pub fn set_pin(&mut self, pin: u32, st: &mut impl Storage) -> SetResult {
        if pin != 0 && !(100_000..=999_999).contains(&pin) {
            return Err(err::ILLEGAL_ARG);
        }
        self.node.prefs.ble_pin = pin;
        self.save_prefs(st);
        let _ = self.actions.push_back(Action::PinChanged);
        Ok(())
    }

    /// `SET_OTHER_PARAMS`: each optional field needs the previous one.
    pub fn set_other_params(
        &mut self,
        manual_add: u8,
        telemetry: Option<u8>,
        advert_loc_policy: Option<u8>,
        multi_acks: Option<u8>,
        st: &mut impl Storage,
    ) {
        let p = &mut self.node.prefs;
        p.manual_add_contacts = manual_add;
        if let Some(t) = telemetry {
            p.telemetry_base = t & 3;
            p.telemetry_loc = (t >> 2) & 3;
            p.telemetry_env = (t >> 4) & 3;
        }
        if let Some(a) = advert_loc_policy {
            p.advert_loc_policy = a;
        }
        if let Some(m) = multi_acks {
            p.multi_acks = m;
        }
        self.save_prefs(st);
    }

    /// `SET_AUTOADD_CONFIG`; hops capped at 64.
    pub fn set_autoadd(&mut self, config: u8, max_hops: Option<u8>, st: &mut impl Storage) {
        self.node.prefs.autoadd_config = config;
        if let Some(h) = max_hops {
            self.node.prefs.autoadd_max_hops = h.min(64);
        }
        self.save_prefs(st);
    }

    /// `SET_PATH_HASH_MODE`: 0 to 2 (1 to 3-byte hashes).
    pub fn set_path_hash_mode(&mut self, mode: u8, st: &mut impl Storage) -> SetResult {
        if mode > 2 {
            return Err(err::ILLEGAL_ARG);
        }
        self.node.prefs.path_hash_mode = mode;
        self.save_prefs(st);
        Ok(())
    }

    /// The `gps` custom variable; only on boards with a GPS.
    pub fn set_gps(&mut self, on: bool, st: &mut impl Storage) -> SetResult {
        if !self.info.has_gps {
            return Err(err::ILLEGAL_ARG);
        }
        self.node.prefs.gps_enabled = on;
        self.save_prefs(st);
        Ok(())
    }

    /// The screen's own settings (prefs record v2); the app never sees them.
    pub fn set_ui(&mut self, ui: UiPrefs, st: &mut impl Storage) {
        self.node.prefs.ui = ui;
        self.save_prefs(st);
    }

    /// `REMOVE_CONTACT`: the contact and its stored advert.
    pub fn remove_contact(
        &mut self,
        now: Instant,
        key: &[u8; 32],
        st: &mut impl Storage,
    ) -> SetResult {
        match self.node.contacts.find(key) {
            Some(Slot::List(i)) => {
                let _ = self.node.contacts.list.remove(i);
                st.remove_blob(key);
                self.mark_contacts(now);
                self.notify(super::UiEvent::Contacts);
                Ok(())
            }
            _ => Err(err::NOT_FOUND),
        }
    }

    /// `RESET_PATH`: the next message to this contact floods and learns a new route.
    pub fn reset_contact_path(&mut self, now: Instant, key: &[u8; 32]) -> SetResult {
        if self.node.reset_path(key) {
            self.mark_contacts(now);
            Ok(())
        } else {
            Err(err::NOT_FOUND)
        }
    }

    /// Favourite: bit 0 of the contact flags (the app shows it too).
    pub fn set_favourite(&mut self, now: Instant, key: &[u8; 32], on: bool) -> SetResult {
        let slot = self.node.contacts.find(key).ok_or(err::NOT_FOUND)?;
        let c = self.node.contacts.get_mut(slot).ok_or(err::NOT_FOUND)?;
        c.flags = (c.flags & !1) | u8::from(on);
        self.mark_contacts(now);
        self.notify(super::UiEvent::Contacts);
        Ok(())
    }
}
