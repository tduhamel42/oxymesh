//! What every board's runner shares (ADR-0009): the radio side of the main loop, the link to
//! the app (USB framing and Bluetooth frames), the boot steps (partitions, identity, import from
//! the C++ firmware) and the Bluetooth host. A board brings its pins, its radio front end, its
//! screen and its buttons.

pub mod boot;
pub mod bt;
pub mod link;
pub mod radio;

/// `mc-core`'s clock: embassy's, in nanoseconds.
pub fn mc_now() -> mc_core::Instant {
    mc_core::Instant::from_nanos(
        embassy_time::Instant::now()
            .as_micros()
            .saturating_mul(1000),
    )
}
