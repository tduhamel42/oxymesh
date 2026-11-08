//! Buzzer tunes (`docs/spec/ui.md` §8): which event plays which tune, and the tunes as notes.
//!
//! The tunes are the C++ firmware's, in RTTTL (`examples/companion_radio/ui-new/UITask.cpp:
//! 770-778`, MIT). The parser takes the subset they and their kind use: `name:d=,o=,b=:` then
//! `[duration]note[#][.][octave][.]`, `p` for a rest. Integers only; nothing allocates.

use mc_node::device::{Device, UiEvent};
use mc_node::history::{Peer, Status};

/// A direct message arrived (C++ `MsgRcv3`).
pub const DM: &str = "MsgRcv3:d=4,o=6,b=200:32e,32g,32b,16c7";
/// A channel message arrived (C++ `kerplop`).
pub const CHANNEL: &str = "kerplop:d=16,o=6,b=120:32g#,32c#";
/// A message sent from the screen was delivered (C++ `ack`).
pub const DELIVERED: &str = "ack:d=32,o=8,b=120:c";

/// What calls for a tune.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Dm,
    Channel,
    Delivered,
}

impl Alert {
    pub fn tune(self) -> &'static str {
        match self {
            Self::Dm => DM,
            Self::Channel => CHANNEL,
            Self::Delivered => DELIVERED,
        }
    }

    /// For the emulator's status line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Dm => "DM",
            Self::Channel => "channel",
            Self::Delivered => "ack",
        }
    }
}

/// How loud (`UiPrefs::buzzer`): 0 off, 1 quiet, 2 on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Quiet,
    Loud,
}

/// The tune an event calls for, if any. Silent when the buzzer is off, while a phone holds
/// the Bluetooth link (the app notifies instead, like the C++), and for muted channels.
pub fn alert(e: &UiEvent, dev: &Device, phone: bool) -> Option<(Alert, Level)> {
    let ui = &dev.node.prefs.ui;
    let level = match ui.buzzer {
        0 => return None,
        1 => Level::Quiet,
        _ => Level::Loud,
    };
    if phone {
        return None;
    }
    let a = match *e {
        UiEvent::Message {
            peer: Peer::Contact(_),
            outgoing: false,
            ..
        } => Alert::Dm,
        UiEvent::Message {
            peer: Peer::Channel(i),
            outgoing: false,
            ..
        } if !ui.is_muted(i) => Alert::Channel,
        UiEvent::Status { seq } => match dev.history().get(seq) {
            Some(m) if m.outgoing && matches!(m.status, Status::Delivered { .. }) => {
                Alert::Delivered
            }
            _ => return None,
        },
        _ => return None,
    };
    Some((a, level))
}

/// One note: frequency (0 = rest) and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub hz: u32,
    pub ms: u32,
}

/// Octave 4, Hz × 100: C C# D D# E F F# G G# A A# B.
const OCTAVE4: [u32; 12] = [
    26163, 27718, 29366, 31113, 32963, 34923, 36999, 39200, 41530, 44000, 46616, 49388,
];

/// The notes of an RTTTL tune. A malformed note ends the tune.
pub fn notes(rtttl: &str) -> Notes<'_> {
    let mut parts = rtttl.splitn(3, ':');
    let _name = parts.next();
    let (mut d, mut o, mut b) = (4u32, 6u32, 63u32);
    for kv in parts.next().unwrap_or("").split(',') {
        let mut it = kv.trim().splitn(2, '=');
        let (k, v) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
        if let Ok(v) = v.trim().parse::<u32>() {
            match k {
                "d" if v > 0 => d = v,
                "o" => o = v,
                "b" if v > 0 => b = v,
                _ => {}
            }
        }
    }
    Notes {
        rest: parts.next().unwrap_or(""),
        d,
        o,
        whole_ms: 240_000 / b,
    }
}

#[derive(Debug, Clone)]
pub struct Notes<'a> {
    rest: &'a str,
    /// Defaults: duration, octave.
    d: u32,
    o: u32,
    /// A whole note, ms.
    whole_ms: u32,
}

impl Iterator for Notes<'_> {
    type Item = Note;

    fn next(&mut self) -> Option<Note> {
        let (tok, rest) = match self.rest.split_once(',') {
            Some((t, r)) => (t, r),
            None => (self.rest, ""),
        };
        self.rest = rest;
        let tok = tok.trim();
        if tok.is_empty() {
            return None;
        }
        let mut s = tok.as_bytes();
        let digits = |s: &mut &[u8]| -> Option<u32> {
            let n = s.iter().take_while(|c| c.is_ascii_digit()).count();
            let (num, tail) = s.split_at(n);
            *s = tail;
            core::str::from_utf8(num).ok()?.parse().ok()
        };
        let dur = digits(&mut s).filter(|&v| v > 0).unwrap_or(self.d);
        let (&letter, tail) = s.split_first()?;
        s = tail;
        let mut semi: Option<usize> = match letter.to_ascii_lowercase() {
            b'c' => Some(0),
            b'd' => Some(2),
            b'e' => Some(4),
            b'f' => Some(5),
            b'g' => Some(7),
            b'a' => Some(9),
            b'b' | b'h' => Some(11),
            b'p' => None,
            _ => return None,
        };
        if let Some((b'#', tail)) = s.split_first() {
            s = tail;
            semi = semi.map(|n| n + 1);
        }
        let mut dotted = false;
        if let Some((b'.', tail)) = s.split_first() {
            s = tail;
            dotted = true;
        }
        let octave = digits(&mut s).unwrap_or(self.o);
        if octave > 8 {
            return None;
        }
        if let Some((b'.', _)) = s.split_first() {
            dotted = true;
        }
        let mut ms = self.whole_ms / dur;
        if dotted {
            ms += ms / 2;
        }
        let hz = match semi {
            None => 0,
            Some(n) => {
                // B# is the next octave's C.
                let (n, octave) = if n == 12 {
                    (0, octave + 1)
                } else {
                    (n, octave)
                };
                let base = *OCTAVE4.get(n)?;
                let x100 = if octave < 4 {
                    base >> (4 - octave)
                } else {
                    base << (octave - 4)
                };
                (x100 + 50) / 100
            }
        };
        Some(Note { hz, ms })
    }
}

/// Length of a tune, ms.
pub fn length_ms(rtttl: &str) -> u32 {
    notes(rtttl).map(|n| n.ms).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn all(t: &str) -> Vec<(u32, u32)> {
        notes(t).map(|n| (n.hz, n.ms)).collect()
    }

    #[test]
    fn the_cpp_tunes() {
        // 200 bpm: a whole note 1200 ms. E6 G6 B6 as 32nds, C7 a 16th.
        assert_eq!(all(DM), [(1319, 37), (1568, 37), (1976, 37), (2093, 75)]);
        // 120 bpm: a whole note 2000 ms. G#6, C#6.
        assert_eq!(all(CHANNEL), [(1661, 62), (1109, 62)]);
        // C8, a 32nd.
        assert_eq!(all(DELIVERED), [(4186, 62)]);
    }

    #[test]
    fn rests_dots_defaults_and_garbage() {
        assert_eq!(
            all("x:d=8,o=5,b=60:p,a.,4a4,c#,b#"),
            [(0, 500), (880, 750), (440, 1000), (554, 500), (1047, 500)]
        );
        assert_eq!(all("x:d=4,o=5,b=60:a,z,a"), [(880, 1000)]);
        assert_eq!(all("x:d=4,o=5,b=60:a9"), []);
        assert_eq!(all(""), []);
        assert_eq!(all("x:b=0:a"), [(1760, 952)]);
        assert_eq!(length_ms(DM), 186);
    }
}
