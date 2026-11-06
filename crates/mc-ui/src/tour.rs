//! Named walks through the interface, as gesture scripts from a fresh `App`: the snapshot
//! tests and `mc ui --png` go through the same screens.

use crate::input::Button::{A, B};
use crate::input::Gesture::{self, Click, Double, Held, Long};

pub const TOUR: &[(&str, &[Gesture])] = &[
    ("home", &[]),
    ("home_launcher_contacts", &[Click(A)]),
    ("messages", &[Long(A)]),
    ("contacts", &[Click(A), Long(A)]),
    ("contact_soon", &[Click(A), Long(A), Long(A)]),
    ("channels", &[Click(A), Click(A), Long(A)]),
    ("tools", &[Click(A), Click(A), Click(A), Long(A)]),
    ("settings", &[Double(A), Long(A)]),
    ("identity", &[Double(A), Long(A), Click(A), Long(A)]),
    ("my_card", &[Double(A), Long(A), Click(A), Long(A), Long(A)]),
    ("system", &[Double(A), Long(A), Double(A), Long(A)]),
    ("about", &[Double(A), Long(A), Double(A), Long(A), Long(A)]),
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
