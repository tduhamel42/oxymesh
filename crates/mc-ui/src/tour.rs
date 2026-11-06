//! Named walks through the interface, as gesture scripts from a fresh `App`: the snapshot
//! tests and `mc ui --png` go through the same screens. The world they expect: two contacts
//! (Alice, Bob), an unread DM from Bob and a channel message from Alice on Public.

use crate::input::Button::{A, B};
use crate::input::Gesture::{self, Click, Double, Held, Long};

const SETTINGS: [Gesture; 2] = [Double(A), Long(A)];

pub const TOUR: &[(&str, &[Gesture])] = &[
    ("home", &[]),
    ("home_launcher_contacts", &[Click(A)]),
    ("messages", &[Long(A)]),
    // Messages: "+ New message" then the conversations, unread first (Bob).
    ("conversation", &[Long(A), Click(A), Long(A)]),
    ("compose", &[Long(A), Click(A), Long(A), Long(A)]),
    // The first candidate of an empty text is the first quick reply; then Send is selected.
    (
        "compose_quick_reply",
        &[Long(A), Click(A), Long(A), Long(A), Long(A)],
    ),
    ("new_message_picker", &[Long(A), Long(A)]),
    ("contacts", &[Click(A), Long(A)]),
    ("contact", &[Click(A), Long(A), Long(A)]),
    ("channels", &[Click(A), Click(A), Long(A)]),
    ("add_channel", &[Click(A), Click(A), Long(A), Long(A)]),
    ("tools", &[Click(A), Click(A), Click(A), Long(A)]),
    (
        "tool_heard",
        &[Click(A), Click(A), Click(A), Long(A), Long(A)],
    ),
    (
        "tool_signal",
        &[
            Click(A),
            Click(A),
            Click(A),
            Long(A),
            Click(A),
            Click(A),
            Click(A),
            Click(A),
            Long(A),
        ],
    ),
    (
        "tool_gps",
        &[
            Click(A),
            Click(A),
            Click(A),
            Long(A),
            Double(A),
            Double(A),
            Long(A),
        ],
    ),
    (
        "tool_map",
        &[Click(A), Click(A), Click(A), Long(A), Double(A), Long(A)],
    ),
    ("settings", &SETTINGS),
    ("identity", &[Double(A), Long(A), Click(A), Long(A)]),
    (
        "name_edit",
        &[Double(A), Long(A), Click(A), Long(A), Click(A), Long(A)],
    ),
    ("my_card", &[Double(A), Long(A), Click(A), Long(A), Long(A)]),
    ("system", &[Double(A), Long(A), Double(A), Long(A)]),
    ("about", &[Double(A), Long(A), Double(A), Long(A), Long(A)]),
    (
        "setup_region",
        &[Double(A), Long(A), Double(A), Long(A), Click(A), Long(A)],
    ),
    (
        "confirm_factory_reset",
        &[
            Double(A),
            Long(A),
            Double(A),
            Long(A),
            Click(A),
            Click(A),
            Click(A),
            Long(A),
            Held(A, 1000),
        ],
    ),
    ("back_home", &[Click(A), Long(A), Double(B)]),
];
