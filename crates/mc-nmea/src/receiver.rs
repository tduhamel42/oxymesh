//! How to power the GPS receivers the boards carry, as steps the firmware carries out on its
//! pins (`docs/spec/m5.md` §6). The ThinkNode M5's board v1.0 has an L76K, driven like the C++
//! (`MicroNMEALocationProvider`, `EnvironmentSensorManager`, MeshCore, MIT); board v2.0 has an
//! Airoha AG3352 behind a 0.8 V rail, which neither the C++ nor anything here has run: its
//! sequence is the hardware fact Meshtastic documents (rail up 100 ms before the UART), and
//! it is used with its default NMEA output.

/// The receiver on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// Quectel L76K (M5 v1.0): EN active high, RESET active low.
    L76k,
    /// Airoha AG3352 (M5 v2.0): a power rail.
    Ag3352,
}

/// A control line of the receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Enable,
    Reset,
    Rail,
}

/// One thing to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Drive a line high (`true`) or low.
    Set(Line, bool),
    /// Wait, ms (short: done in place).
    Wait(u64),
    /// Send this sentence (without `$`, `*` and checksum) this long after the steps before it,
    /// ms (long: the firmware schedules it and goes on).
    Later(u64, &'static [u8]),
}

impl Receiver {
    /// Switching on.
    pub const fn on(self) -> &'static [Step] {
        match self {
            // EN high, RESET pulsed, then GPS + BeiDou + GLONASS 250 ms later (`configureL76K`).
            Receiver::L76k => &[
                Step::Set(Line::Enable, true),
                Step::Set(Line::Reset, false),
                Step::Wait(10),
                Step::Set(Line::Reset, true),
                Step::Later(250, b"PCAS04,7"),
            ],
            Receiver::Ag3352 => &[Step::Set(Line::Rail, true), Step::Wait(100)],
        }
    }

    /// Switching off.
    pub const fn off(self) -> &'static [Step] {
        match self {
            Receiver::L76k => &[
                Step::Set(Line::Enable, false),
                Step::Set(Line::Reset, false),
            ],
            Receiver::Ag3352 => &[Step::Set(Line::Rail, false)],
        }
    }

    /// The lines it has.
    pub const fn lines(self) -> &'static [Line] {
        match self {
            Receiver::L76k => &[Line::Enable, Line::Reset],
            Receiver::Ag3352 => &[Line::Rail],
        }
    }
}

/// `$` + payload + `*` + checksum (XOR of the payload, two uppercase hex digits) + CRLF.
/// Returns the length written; a sentence that does not fit is cut.
pub fn sentence(payload: &[u8], out: &mut [u8]) -> usize {
    let sum = payload.iter().fold(0u8, |a, b| a ^ b);
    let hex = |v: u8| if v < 10 { b'0' + v } else { b'A' + v - 10 };
    let tail = [b'*', hex(sum >> 4), hex(sum & 15), b'\r', b'\n'];
    let mut n = 0;
    for &b in b"$".iter().chain(payload).chain(&tail) {
        let Some(o) = out.get_mut(n) else { break };
        *o = b;
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l76k_like_the_cpp() {
        assert_eq!(
            Receiver::L76k.on(),
            &[
                Step::Set(Line::Enable, true),
                Step::Set(Line::Reset, false),
                Step::Wait(10),
                Step::Set(Line::Reset, true),
                Step::Later(250, b"PCAS04,7"),
            ]
        );
        let mut b = [0u8; 32];
        let n = sentence(b"PCAS04,7", &mut b);
        assert_eq!(&b[..n], b"$PCAS04,7*1E\r\n");
    }

    #[test]
    fn ag3352_rail_comes_first() {
        let on = Receiver::Ag3352.on();
        assert_eq!(on.first(), Some(&Step::Set(Line::Rail, true)));
        assert_eq!(on.get(1), Some(&Step::Wait(100)));
        // Nothing sent: its default NMEA output is used.
        assert!(!on.iter().any(|s| matches!(s, Step::Later(..))));
        assert_eq!(Receiver::Ag3352.off(), &[Step::Set(Line::Rail, false)]);
    }

    #[test]
    fn steps_use_only_lines_the_receiver_has() {
        for r in [Receiver::L76k, Receiver::Ag3352] {
            for s in r.on().iter().chain(r.off()) {
                if let Step::Set(l, _) = s {
                    assert!(r.lines().contains(l), "{r:?} {l:?}");
                }
            }
        }
    }
}
