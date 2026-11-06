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
#[derive(Debug, Clone)]
pub enum Action {
    /// A screen built from the current state (a choice with the current value marked).
    Build(fn(&Ui<'_>) -> Box<dyn Screen>),
    /// Something done at once (a toggle).
    Run(fn(&Ui<'_>) -> Nav),
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

impl Item {
    pub fn plain(label: &'static str, action: Action) -> Item {
        item(label, action)
    }

    pub fn with(label: &'static str, detail: fn(&Ui<'_>) -> String, action: Action) -> Item {
        item_with(label, detail, action)
    }
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

    fn input(&mut self, input: Input, ui: &Ui<'_>) -> Nav {
        if !step(&mut self.state, self.items.len(), input) {
            return Nav::Stay;
        }
        let Some(item) = self.items.get(self.selected()) else {
            return Nav::Stay;
        };
        match item.action.clone() {
            Action::Build(f) => Nav::Push(f(ui)),
            Action::Run(f) => f(ui),
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
            item("Heard nodes", Action::Open(Dest::Heard)),
            item("Discover", Action::Open(Dest::Discover)),
            item("Trace route", Action::Open(Dest::TracePick)),
            item("Ping & range", Action::Open(Dest::RangePick)),
            item("Signal", Action::Open(Dest::Signal)),
            item("Packet log", Action::Open(Dest::Packets)),
            item("Airtime", Action::Open(Dest::Airtime)),
            item("Repeater admin", Action::Open(Dest::AdminPick)),
            item("GPS", Action::Open(Dest::Gps)),
            item("Map", Action::Open(Dest::Map)),
        ],
    )
}

pub fn settings() -> Menu {
    Menu::new(
        "Settings",
        &[
            item_with("Radio", radio, Action::Open(Dest::Radio)),
            item_with("Identity", name, Action::Open(Dest::Identity)),
            item("Bluetooth", Action::Open(Dest::Bluetooth)),
            item("Contacts", Action::Open(Dest::ContactSettings)),
            item("Display", Action::Open(Dest::Display)),
            item("Notifications", Action::Open(Dest::Notifications)),
            item("Telemetry", Action::Open(Dest::Telemetry)),
            item("Power", Action::Open(Dest::Power)),
            item_with(
                "Time",
                |ui| super::settings::utc(ui.ctx.dev.node.prefs.ui.tz_offset_min),
                Action::Open(Dest::Time),
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
            item_with(
                "Share location",
                super::settings::location_state,
                Action::Run(super::settings::location_toggle),
            ),
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
