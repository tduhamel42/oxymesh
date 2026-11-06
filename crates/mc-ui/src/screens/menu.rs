//! A list of choices: Tools, Settings and their sub-menus.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ratatui_core::layout::Rect;
use ratatui_core::terminal::Frame;
use ratatui_widgets::list::ListState;

use super::{Dest, Soon, draw_list, open, step};
use crate::act::Intent;
use crate::app::{Confirm, Input, Nav, Screen, Ui};

/// What choosing an item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Open(Dest),
    /// Not built yet: a screen that says which milestone brings it.
    Soon(&'static str),
    Intent(Intent, &'static str),
    /// Hold-to-confirm, then the intent and a toast.
    Confirm {
        text: &'static str,
        intent: Intent,
        toast: &'static str,
    },
}

#[derive(Debug, Clone)]
pub struct Item {
    pub label: &'static str,
    /// Value shown on the right, computed when drawn.
    pub detail: Option<fn(&Ui<'_>) -> String>,
    pub action: Action,
}

const fn item(label: &'static str, action: Action) -> Item {
    Item {
        label,
        detail: None,
        action,
    }
}

const fn item_with(label: &'static str, detail: fn(&Ui<'_>) -> String, action: Action) -> Item {
    Item {
        label,
        detail: Some(detail),
        action,
    }
}

#[derive(Debug)]
pub struct Menu {
    title: &'static str,
    items: Vec<Item>,
    state: ListState,
}

impl Menu {
    pub fn new(title: &'static str, items: &[Item]) -> Self {
        Self {
            title,
            items: items.to_vec(),
            state: ListState::default().with_selected(Some(0)),
        }
    }

    pub fn selected(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }
}

impl Screen for Menu {
    fn title(&self, _ui: &Ui<'_>) -> String {
        String::from(self.title)
    }

    fn draw(&mut self, f: &mut Frame<'_>, body: Rect, ui: &Ui<'_>) {
        let rows: Vec<(String, String)> = self
            .items
            .iter()
            .map(|i| {
                (
                    String::from(i.label),
                    i.detail.map(|d| d(ui)).unwrap_or_default(),
                )
            })
            .collect();
        draw_list(f, body, &rows, &mut self.state, ui);
    }

    fn input(&mut self, input: Input, _ui: &Ui<'_>) -> Nav {
        if !step(&mut self.state, self.items.len(), input) {
            return Nav::Stay;
        }
        let Some(item) = self.items.get(self.selected()) else {
            return Nav::Stay;
        };
        match item.action.clone() {
            Action::Open(d) => Nav::Push(open(d)),
            Action::Soon(note) => Nav::Push(Box::new(Soon::new(item.label, note))),
            Action::Intent(i, toast) => Nav::IntentToast(i, toast),
            Action::Confirm {
                text,
                intent,
                toast,
            } => Nav::Confirm(Confirm::new(item.label, text, intent).with_toast(toast)),
        }
    }
}

fn radio(ui: &Ui<'_>) -> String {
    let r = &ui.ctx.dev.node.prefs.radio;
    format!("SF{} {}.{:03}", r.sf, r.freq_khz / 1000, r.freq_khz % 1000)
}

fn name(ui: &Ui<'_>) -> String {
    super::name_of(&ui.ctx.dev.node.prefs.name)
}

pub fn tools() -> Menu {
    Menu::new(
        "Tools",
        &[
            item(
                "Heard nodes",
                Action::Soon("Nodes heard, signal, hops and distance: arrives in U4."),
            ),
            item(
                "Discover",
                Action::Soon("Zero-hop discovery with SNR both ways: arrives in U4."),
            ),
            item(
                "Trace route",
                Action::Soon("Per-hop SNR in both directions: arrives in U4."),
            ),
            item(
                "Ping & range",
                Action::Soon("Repeated pings, success rate, RTT and SNR: arrives in U4."),
            ),
            item(
                "Signal",
                Action::Soon("RSSI, SNR, noise floor and packet rate: arrives in U4."),
            ),
            item(
                "Packet log",
                Action::Soon("Recent packets, decoded: arrives in U4."),
            ),
            item(
                "Airtime",
                Action::Soon("Airtime and the duty-cycle budget: arrives in U4."),
            ),
            item(
                "Repeater admin",
                Action::Soon("Login, status and neighbours: arrives in U4."),
            ),
            item(
                "GPS",
                Action::Soon("Fix, satellites, position: arrives in U4."),
            ),
            item(
                "Map",
                Action::Soon("You and your contacts on a map: arrives in U4."),
            ),
        ],
    )
}

pub fn settings() -> Menu {
    Menu::new(
        "Settings",
        &[
            item_with(
                "Radio",
                radio,
                Action::Soon("Region, frequency, SF, bandwidth, power: arrives in U5."),
            ),
            item_with("Identity", name, Action::Open(Dest::Identity)),
            item(
                "Bluetooth",
                Action::Soon("Pairing and PIN: arrive with Bluetooth (MC5)."),
            ),
            item(
                "Contacts",
                Action::Soon("Auto-add rules and limits: arrive in U5."),
            ),
            item(
                "Display",
                Action::Soon("Light timeout, text size, invert, rotate: arrive in U5."),
            ),
            item(
                "Notifications",
                Action::Soon("Buzzer and LED: arrive in U5."),
            ),
            item(
                "Telemetry",
                Action::Soon("Who may read battery, location, sensors: arrives in U5."),
            ),
            item(
                "Power",
                Action::Soon("GPS power and hibernate: arrive in U5."),
            ),
            item(
                "Time",
                Action::Soon("Time source and time zone: arrive in U5."),
            ),
            item("System", Action::Open(Dest::System)),
        ],
    )
}

pub fn identity() -> Menu {
    Menu::new(
        "Identity",
        &[
            item("My card (QR)", Action::Open(Dest::MyCard)),
            item_with("Name", name, Action::Open(Dest::Name)),
            item(
                "Advert, zero hop",
                Action::Intent(Intent::SendAdvert { flood: false }, "Advert sent"),
            ),
            item(
                "Advert, flood",
                Action::Confirm {
                    text: "Every repeater around will forward it.",
                    intent: Intent::SendAdvert { flood: true },
                    toast: "Advert flooded",
                },
            ),
        ],
    )
}

pub fn system() -> Menu {
    Menu::new(
        "System",
        &[
            item("About", Action::Open(Dest::About)),
            item("Setup again", Action::Open(Dest::Setup)),
            item(
                "Reboot",
                Action::Confirm {
                    text: "Restart the device.",
                    intent: Intent::Reboot,
                    toast: "Rebooting",
                },
            ),
            item(
                "Factory reset",
                Action::Confirm {
                    text: "Erase the identity, contacts, channels and settings.",
                    intent: Intent::FactoryReset,
                    toast: "Erasing",
                },
            ),
            item(
                "Import from C++",
                Action::Soon("Import from the MeshCore C++ firmware arrives in MC6."),
            ),
        ],
    )
}
