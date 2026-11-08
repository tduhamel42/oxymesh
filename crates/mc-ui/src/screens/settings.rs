//! Settings (`docs/spec/ui.md` §6): every value through the device's settings API, so the app
//! sees the same thing in `SELF_INFO`. Radio changes and turning off ask for hold-to-confirm.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mc_node::{Prefs, UiPrefs};
use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_widgets::list::ListState;

use super::chat::{Compose, Target};
use super::menu::{Action, Item, Menu};
use super::{Dest, draw_list, step};
use crate::act::{Intent, Outcome};
use crate::app::{BleState, Confirm, Input, Nav, Screen, Ui};
use crate::{clock, regions};

fn prefs<'a>(ui: &Ui<'a>) -> &'a Prefs {
    &ui.ctx.dev.node.prefs
}

// ------------------------------------------------------------------ choosing one value

/// Pick one of several values; the current one is marked, and comes first in long lists.
#[derive(Debug)]
pub struct Choice {
    title: String,
    options: Vec<(String, Intent)>,
    current: Option<usize>,
    /// Hold-to-confirm with this text before applying.
    confirm: Option<String>,
    toast: &'static str,
    state: ListState,
}

impl Choice {
    pub fn new(title: &str, mut options: Vec<(String, Intent)>, current: Option<usize>) -> Self {
        let mut cur = current;
        if options.len() > 8
            && let Some(i) = current
            && i < options.len()
        {
            let o = options.remove(i);
            options.insert(0, o);
            cur = Some(0);
        }
        Self {
            title: String::from(title),
            options,
            current: cur,
            confirm: None,
            toast: "Saved",
            state: ListState::default().with_selected(Some(cur.unwrap_or(0))),
        }
    }

    pub fn confirm(mut self, text: &str) -> Self {
        self.confirm = Some(String::from(text));
        self
    }
}

impl Screen for Choice {
    fn title(&self, _ui: &Ui<'_>) -> String {
        self.title.clone()
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let rows: Vec<(String, String)> = self
            .options
            .iter()
            .enumerate()
            .map(|(i, (l, _))| {
                let mark = if Some(i) == self.current {
                    String::from(ui.sym.unread)
                } else {
                    String::new()
                };
                (l.clone(), mark)
            })
            .collect();
        draw_list(f, body, &rows, &mut self.state, ui);
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        if !step(&mut self.state, self.options.len(), input) {
            return Nav::Stay;
        }
        let i = self.state.selected().unwrap_or(0);
        let Some((label, intent)) = self.options.get(i).cloned() else {
            return Nav::Stay;
        };
        if Some(i) == self.current {
            return Nav::Pop;
        }
        match &self.confirm {
            Some(text) => Nav::Confirm(
                Confirm::new(&self.title, &format!("{text} {label}."), intent)
                    .with_toast(self.toast),
            ),
            None => Nav::PopWith(intent, Some(self.toast)),
        }
    }

    fn result(&mut self, _intent: &Intent, outcome: &Outcome, _ui: &Ui<'_>) -> Nav {
        // After a confirmed change: back to the menu.
        if self.confirm.is_some() && *outcome == Outcome::Ok {
            return Nav::Pop;
        }
        Nav::Stay
    }
}

// ------------------------------------------------------------------ helpers

fn ui_with(ui: &Ui<'_>, f: impl FnOnce(&mut UiPrefs)) -> Intent {
    let mut u = prefs(ui).ui;
    f(&mut u);
    Intent::SetUi(u)
}

/// `SET_OTHER_PARAMS` from the current values with one change.
fn other_with(ui: &Ui<'_>, f: impl FnOnce(&mut [u8; 4])) -> Intent {
    let p = prefs(ui);
    let mut v = [
        p.manual_add_contacts,
        p.telemetry_base | (p.telemetry_loc << 2) | (p.telemetry_env << 4),
        p.advert_loc_policy,
        p.multi_acks,
    ];
    f(&mut v);
    Intent::SetOther {
        manual_add: v[0],
        telemetry: v[1],
        advert_loc: v[2],
        multi_acks: v[3],
    }
}

fn khz(hz: u32) -> String {
    if hz.is_multiple_of(1000) {
        format!("{} kHz", hz / 1000)
    } else {
        format!("{}.{} kHz", hz / 1000, hz % 1000 / 100)
    }
}

fn freq(khz_: u32) -> String {
    format!("{}.{:03}", khz_ / 1000, khz_ % 1000)
}

fn yes(b: bool) -> String {
    String::from(if b { "on" } else { "off" })
}

const RADIO_CONFIRM: &str = "Every node you talk to must use the same setting. Switch to";

// ------------------------------------------------------------------ radio

fn radio_change(ui: &Ui<'_>, f: impl FnOnce(&mut (u32, u32, u8, u8))) -> Intent {
    let r = &prefs(ui).radio;
    let mut v = (r.freq_khz, r.bw_hz, r.sf, r.cr);
    f(&mut v);
    Intent::SetRadio {
        freq_khz: v.0,
        bw_hz: v.1,
        sf: v.2,
        cr: v.3,
    }
}

fn region_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let r = &prefs(ui).radio;
    let presets = regions::presets();
    let current = presets.iter().position(|g| {
        g.freq_khz == r.freq_khz && g.bw_hz == r.bw_hz && g.sf == r.sf && g.cr == r.cr
    });
    let options = presets
        .iter()
        .map(|g| {
            (
                format!("{} {} SF{}", g.name, freq(g.freq_khz), g.sf),
                Intent::SetRadio {
                    freq_khz: g.freq_khz,
                    bw_hz: g.bw_hz,
                    sf: g.sf,
                    cr: g.cr,
                },
            )
        })
        .collect();
    Box::new(Choice::new("Region", options, current).confirm(RADIO_CONFIRM))
}

const BANDWIDTHS: [u32; 10] = [
    7_800, 10_400, 15_600, 20_800, 31_250, 41_700, 62_500, 125_000, 250_000, 500_000,
];

fn bandwidth_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = prefs(ui).radio.bw_hz;
    let options = BANDWIDTHS
        .iter()
        .map(|&b| (khz(b), radio_change(ui, |v| v.1 = b)))
        .collect();
    let current = BANDWIDTHS.iter().position(|&b| b == cur);
    Box::new(Choice::new("Bandwidth", options, current).confirm(RADIO_CONFIRM))
}

fn sf_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = prefs(ui).radio.sf;
    let options = (5..=12u8)
        .map(|s| (format!("SF{s}"), radio_change(ui, |v| v.2 = s)))
        .collect();
    Box::new(
        Choice::new(
            "Spreading factor",
            options,
            Some(usize::from(cur.saturating_sub(5))),
        )
        .confirm(RADIO_CONFIRM),
    )
}

fn cr_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = prefs(ui).radio.cr;
    let options = (5..=8u8)
        .map(|c| (format!("4/{c}"), radio_change(ui, |v| v.3 = c)))
        .collect();
    Box::new(
        Choice::new(
            "Coding rate",
            options,
            Some(usize::from(cur.saturating_sub(5))),
        )
        .confirm(RADIO_CONFIRM),
    )
}

fn power_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let max = ui.ctx.dev.info.max_tx_power;
    let cur = prefs(ui).radio.tx_power_dbm;
    let values: Vec<i8> = (-9..=max).rev().collect();
    let options = values
        .iter()
        .map(|&d| (format!("{d} dBm"), Intent::SetTxPower(d)))
        .collect();
    let current = values.iter().position(|&d| d == cur);
    Box::new(Choice::new("Transmit power", options, current))
}

fn hash_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = prefs(ui).path_hash_mode;
    let options = (0..=2u8)
        .map(|m| {
            (
                format!("{} byte{}", m + 1, if m == 0 { "" } else { "s" }),
                Intent::SetPathHash(m),
            )
        })
        .collect();
    Box::new(Choice::new(
        "Path hash size",
        options,
        Some(usize::from(cur)),
    ))
}

fn scope_name(p: &Prefs) -> String {
    super::name_of(&p.default_scope_name)
}

fn scope_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = scope_name(prefs(ui));
    let mut options = alloc::vec![(String::from("None (everywhere)"), Intent::SetScope(None))];
    let mut current = cur.is_empty().then_some(0);
    for g in regions::presets() {
        if options.iter().any(|(l, _)| *l == g.id) {
            continue;
        }
        if g.id == cur {
            current = Some(options.len());
        }
        options.push((g.id.clone(), Intent::SetScope(Some(g.id.clone()))));
    }
    if current.is_none() && !cur.is_empty() {
        current = Some(options.len());
        options.push((cur.clone(), Intent::SetScope(Some(cur))));
    }
    Box::new(Choice::new("Flood scope", options, current))
}

fn frequency_entry(ui: &Ui<'_>) -> Box<dyn Screen> {
    Box::new(Compose::new(
        Target::Frequency,
        &format!("{}", prefs(ui).radio.freq_khz),
    ))
}

pub fn radio() -> Menu {
    Menu::new(
        "Radio",
        &[
            Item::with(
                "Region",
                |ui| {
                    let r = &prefs(ui).radio;
                    regions::presets()
                        .into_iter()
                        .find(|g| {
                            g.freq_khz == r.freq_khz
                                && g.bw_hz == r.bw_hz
                                && g.sf == r.sf
                                && g.cr == r.cr
                        })
                        .map(|g| g.name)
                        .unwrap_or_else(|| String::from("custom"))
                },
                Action::Build(region_choice),
            ),
            Item::with(
                "Frequency",
                |ui| freq(prefs(ui).radio.freq_khz),
                Action::Build(frequency_entry),
            ),
            Item::with(
                "Bandwidth",
                |ui| khz(prefs(ui).radio.bw_hz),
                Action::Build(bandwidth_choice),
            ),
            Item::with(
                "Spreading",
                |ui| format!("SF{}", prefs(ui).radio.sf),
                Action::Build(sf_choice),
            ),
            Item::with(
                "Coding rate",
                |ui| format!("4/{}", prefs(ui).radio.cr),
                Action::Build(cr_choice),
            ),
            Item::with(
                "Power",
                |ui| format!("{} dBm", prefs(ui).radio.tx_power_dbm),
                Action::Build(power_choice),
            ),
            Item::with(
                "Path hash",
                |ui| format!("{} B", prefs(ui).path_hash_mode + 1),
                Action::Build(hash_choice),
            ),
            Item::with(
                "Flood scope",
                |ui| {
                    let n = scope_name(prefs(ui));
                    if n.is_empty() {
                        String::from("none")
                    } else {
                        n
                    }
                },
                Action::Build(scope_choice),
            ),
        ],
    )
}

// ------------------------------------------------------------------ bluetooth

fn pin_entry(_ui: &Ui<'_>) -> Box<dyn Screen> {
    Box::new(Compose::new(Target::Pin, ""))
}

pub fn bluetooth() -> Menu {
    Menu::new(
        "Bluetooth",
        &[
            Item::with(
                "Status",
                |ui| {
                    String::from(match ui.ctx.ble.state {
                        BleState::Off => "off",
                        BleState::Advertising => "waiting",
                        BleState::Pairing => "pairing",
                        BleState::Connected => "connected",
                    })
                },
                Action::Run(|_| Nav::Stay),
            ),
            Item::with(
                "PIN",
                |ui| match (prefs(ui).ble_pin, ui.ctx.ble.pin) {
                    (0, 0) => String::from("random"),
                    (0, p) => format!("{p:06} random"),
                    (p, _) => format!("{p:06}"),
                },
                Action::Build(pin_entry),
            ),
            Item::plain(
                "New random PIN",
                Action::Run(|_| Nav::IntentToast(Intent::SetPin(0), "New random PIN")),
            ),
            Item::with(
                "Paired phones",
                |ui| format!("{}", ui.ctx.ble.paired),
                Action::Confirm {
                    text: "Forget every paired phone? Each will have to pair again.",
                    intent: Intent::ForgetPaired,
                    toast: "Paired phones forgotten",
                },
            ),
        ],
    )
}

// ------------------------------------------------------------------ contacts

const TYPE_BITS: [(u8, &str); 4] = [
    (0x02, "chat"),
    (0x04, "repeater"),
    (0x08, "room"),
    (0x10, "sensor"),
];

fn autoadd_with(ui: &Ui<'_>, f: impl FnOnce(&mut u8, &mut u8)) -> Intent {
    let p = prefs(ui);
    let (mut c, mut h) = (p.autoadd_config, p.autoadd_max_hops);
    f(&mut c, &mut h);
    Intent::SetAutoadd {
        config: c,
        max_hops: h,
    }
}

fn add_mode_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = usize::from(prefs(ui).manual_add_contacts & 1);
    let options = alloc::vec![
        (
            String::from("Every node heard"),
            other_with(ui, |v| v[0] &= !1)
        ),
        (
            String::from("Only the types below"),
            other_with(ui, |v| v[0] |= 1)
        ),
    ];
    Box::new(Choice::new("Add contacts", options, Some(cur)))
}

fn hops_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = prefs(ui).autoadd_max_hops;
    let values = [0u8, 1, 2, 3, 4, 6, 8, 16];
    let options = values
        .iter()
        .map(|&h| {
            (
                if h == 0 {
                    String::from("No limit")
                } else {
                    format!("{h} hops")
                },
                autoadd_with(ui, |_, m| *m = h),
            )
        })
        .collect();
    Box::new(Choice::new(
        "Max hops",
        options,
        values.iter().position(|&h| h == cur),
    ))
}

fn type_toggle(ui: &Ui<'_>, bit: u8, name: &str) -> Nav {
    let on = prefs(ui).autoadd_config & bit == 0;
    let _ = name;
    Nav::IntentToast(
        autoadd_with(ui, |c, _| *c ^= bit),
        if on {
            "Added automatically"
        } else {
            "Not added automatically"
        },
    )
}

pub fn contacts() -> Menu {
    Menu::new(
        "Contacts",
        &[
            Item::with(
                "Add",
                |ui| {
                    String::from(if prefs(ui).manual_add_contacts & 1 == 0 {
                        "all"
                    } else {
                        "by type"
                    })
                },
                Action::Build(add_mode_choice),
            ),
            Item::with(
                "Chat",
                |ui| yes(prefs(ui).autoadd_config & TYPE_BITS[0].0 != 0),
                Action::Run(|ui| type_toggle(ui, TYPE_BITS[0].0, TYPE_BITS[0].1)),
            ),
            Item::with(
                "Repeater",
                |ui| yes(prefs(ui).autoadd_config & TYPE_BITS[1].0 != 0),
                Action::Run(|ui| type_toggle(ui, TYPE_BITS[1].0, TYPE_BITS[1].1)),
            ),
            Item::with(
                "Room",
                |ui| yes(prefs(ui).autoadd_config & TYPE_BITS[2].0 != 0),
                Action::Run(|ui| type_toggle(ui, TYPE_BITS[2].0, TYPE_BITS[2].1)),
            ),
            Item::with(
                "Sensor",
                |ui| yes(prefs(ui).autoadd_config & TYPE_BITS[3].0 != 0),
                Action::Run(|ui| type_toggle(ui, TYPE_BITS[3].0, TYPE_BITS[3].1)),
            ),
            Item::with(
                "When full",
                |ui| {
                    String::from(if prefs(ui).autoadd_config & 1 != 0 {
                        "replace oldest"
                    } else {
                        "keep"
                    })
                },
                Action::Run(|ui| {
                    let on = prefs(ui).autoadd_config & 1 == 0;
                    Nav::IntentToast(
                        autoadd_with(ui, |c, _| *c ^= 1),
                        if on {
                            "Oldest replaced when full"
                        } else {
                            "Kept when full"
                        },
                    )
                }),
            ),
            Item::with(
                "Max hops",
                |ui| match prefs(ui).autoadd_max_hops {
                    0 => String::from("no limit"),
                    h => format!("{h}"),
                },
                Action::Build(hops_choice),
            ),
        ],
    )
}

// ------------------------------------------------------------------ display, notifications

const TIMEOUTS: [u16; 4] = [30, 120, 600, 0];

fn timeout_text(s: u16) -> String {
    match s {
        0 => String::from("never"),
        s if s < 60 => format!("{s} s"),
        s => format!("{} min", s / 60),
    }
}

fn size_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = usize::from(prefs(ui).ui.compact);
    let options = alloc::vec![
        (
            String::from("Large (25×15)"),
            ui_with(ui, |u| u.compact = false)
        ),
        (
            String::from("Compact (33×20)"),
            ui_with(ui, |u| u.compact = true)
        ),
    ];
    Box::new(Choice::new("Text size", options, Some(cur)))
}

fn timeout_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = prefs(ui).ui.light_timeout_s;
    let options = TIMEOUTS
        .iter()
        .map(|&t| (timeout_text(t), ui_with(ui, |u| u.light_timeout_s = t)))
        .collect();
    Box::new(Choice::new(
        "Light off after",
        options,
        TIMEOUTS.iter().position(|&t| t == cur),
    ))
}

pub fn display() -> Menu {
    Menu::new(
        "Display",
        &[
            Item::with(
                "Text size",
                |ui| {
                    String::from(if prefs(ui).ui.compact {
                        "compact"
                    } else {
                        "large"
                    })
                },
                Action::Build(size_choice),
            ),
            Item::with(
                "Light off",
                |ui| timeout_text(prefs(ui).ui.light_timeout_s),
                Action::Build(timeout_choice),
            ),
            Item::with(
                "Invert",
                |ui| yes(prefs(ui).ui.invert),
                Action::Run(|ui| Nav::IntentToast(ui_with(ui, |u| u.invert = !u.invert), "Saved")),
            ),
            Item::with(
                "Rotate 180°",
                |ui| yes(prefs(ui).ui.rotate),
                Action::Run(|ui| Nav::IntentToast(ui_with(ui, |u| u.rotate = !u.rotate), "Saved")),
            ),
        ],
    )
}

fn buzzer_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = usize::from(prefs(ui).ui.buzzer.min(2));
    let options = ["Off", "Quiet", "On"]
        .iter()
        .enumerate()
        .map(|(i, l)| (String::from(*l), ui_with(ui, |u| u.buzzer = i as u8)))
        .collect();
    Box::new(Choice::new("Buzzer", options, Some(cur)))
}

pub fn notifications() -> Menu {
    Menu::new(
        "Notifications",
        &[
            Item::with(
                "Buzzer",
                |ui| String::from(["off", "quiet", "on"][usize::from(prefs(ui).ui.buzzer.min(2))]),
                Action::Build(buzzer_choice),
            ),
            Item::with(
                "LED on unread",
                |ui| yes(prefs(ui).ui.led_unread),
                Action::Run(|ui| {
                    Nav::IntentToast(ui_with(ui, |u| u.led_unread = !u.led_unread), "Saved")
                }),
            ),
        ],
    )
}

// ------------------------------------------------------------------ telemetry

const WHO: [&str; 3] = ["nobody", "favourites", "everyone"];

/// `shift`: 0 battery, 2 location, 4 environment.
fn telemetry_choice(ui: &Ui<'_>, shift: u8, title: &str) -> Box<dyn Screen> {
    let p = prefs(ui);
    let tel = p.telemetry_base | (p.telemetry_loc << 2) | (p.telemetry_env << 4);
    let cur = usize::from((tel >> shift) & 3).min(2);
    let options = WHO
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let mut s = String::from(*w);
            if let Some(c) = s.get_mut(..1) {
                c.make_ascii_uppercase();
            }
            (
                s,
                other_with(ui, |v| v[1] = (v[1] & !(3 << shift)) | ((i as u8) << shift)),
            )
        })
        .collect();
    Box::new(Choice::new(title, options, Some(cur)))
}

pub fn telemetry() -> Menu {
    Menu::new(
        "Telemetry",
        &[
            Item::with(
                "Battery",
                |ui| String::from(WHO[usize::from(prefs(ui).telemetry_base.min(2))]),
                Action::Build(|ui| telemetry_choice(ui, 0, "Battery: who may read")),
            ),
            Item::with(
                "Location",
                |ui| String::from(WHO[usize::from(prefs(ui).telemetry_loc.min(2))]),
                Action::Build(|ui| telemetry_choice(ui, 2, "Location: who may read")),
            ),
            Item::with(
                "Sensors",
                |ui| String::from(WHO[usize::from(prefs(ui).telemetry_env.min(2))]),
                Action::Build(|ui| telemetry_choice(ui, 4, "Sensors: who may read")),
            ),
        ],
    )
}

// ------------------------------------------------------------------ power, time

/// GPS position updates: (seconds, label); 0 is every second, like the C++.
const GPS_INTERVALS: [(u32, &str); 7] = [
    (0, "1 s"),
    (10, "10 s"),
    (30, "30 s"),
    (60, "1 min"),
    (300, "5 min"),
    (900, "15 min"),
    (3600, "1 h"),
];

fn interval_label(s: u32) -> String {
    match GPS_INTERVALS.iter().find(|(v, _)| *v == s) {
        Some((_, l)) => String::from(*l),
        None => format!("{s} s"),
    }
}

fn gps_interval_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = prefs(ui).gps_interval;
    let options = GPS_INTERVALS
        .iter()
        .map(|(s, l)| (String::from(*l), Intent::SetGpsInterval(*s)))
        .collect();
    let current = GPS_INTERVALS.iter().position(|(s, _)| *s == cur);
    Box::new(Choice::new("GPS updates", options, current))
}

pub fn power() -> Menu {
    Menu::new(
        "Power",
        &[
            Item::with(
                "GPS",
                |ui| yes(prefs(ui).gps_enabled),
                Action::Run(|ui| {
                    let on = !prefs(ui).gps_enabled;
                    Nav::IntentToast(Intent::SetGps(on), if on { "GPS on" } else { "GPS off" })
                }),
            ),
            Item::with(
                "GPS updates",
                |ui| interval_label(prefs(ui).gps_interval),
                Action::Build(gps_interval_choice),
            ),
            Item::plain(
                "Hibernate",
                Action::Run(|_| {
                    Nav::Confirm(Confirm::new(
                        "Hibernate",
                        "Turn off the radio and the screen until B is pressed.",
                        Intent::Hibernate,
                    ))
                }),
            ),
        ],
    )
}

/// `UTC+1`, `UTC-3:30`.
pub fn utc(min: i16) -> String {
    let sign = if min < 0 { '-' } else { '+' };
    let a = min.unsigned_abs();
    if a.is_multiple_of(60) {
        format!("UTC{sign}{}", a / 60)
    } else {
        format!("UTC{sign}{}:{:02}", a / 60, a % 60)
    }
}

fn tz_choice(ui: &Ui<'_>) -> Box<dyn Screen> {
    let cur = prefs(ui).ui.tz_offset_min;
    let values: Vec<i16> = (-24..=28).map(|h| h * 30).collect();
    let options = values
        .iter()
        .map(|&m| (utc(m), ui_with(ui, |u| u.tz_offset_min = m)))
        .collect();
    Box::new(Choice::new(
        "Time zone",
        options,
        values.iter().position(|&m| m == cur),
    ))
}

pub fn time() -> Menu {
    Menu::new(
        "Time",
        &[
            Item::with(
                "Time zone",
                |ui| utc(prefs(ui).ui.tz_offset_min),
                Action::Build(tz_choice),
            ),
            Item::with(
                "Clock",
                |ui| clock::hhmm(clock::local(ui.ctx.unix, prefs(ui).ui.tz_offset_min)),
                Action::Run(|_| Nav::Toast(String::from("Set by the phone app or the GPS"))),
            ),
        ],
    )
}

/// Settings → Identity → Share location: in adverts or not.
pub fn location_toggle(ui: &Ui<'_>) -> Nav {
    let on = prefs(ui).advert_loc_policy == 0;
    Nav::IntentToast(
        other_with(ui, |v| v[2] = u8::from(on)),
        if on {
            "Location in adverts"
        } else {
            "Location not shared"
        },
    )
}

pub fn location_state(ui: &Ui<'_>) -> String {
    yes(prefs(ui).advert_loc_policy != 0)
}

/// For menus pointing here.
pub fn open(d: Dest) -> Option<Box<dyn Screen>> {
    Some(match d {
        Dest::Radio => Box::new(radio()),
        Dest::Bluetooth => Box::new(bluetooth()),
        Dest::ContactSettings => Box::new(contacts()),
        Dest::Display => Box::new(display()),
        Dest::Notifications => Box::new(notifications()),
        Dest::Telemetry => Box::new(telemetry()),
        Dest::Power => Box::new(power()),
        Dest::Time => Box::new(time()),
        _ => return None,
    })
}
