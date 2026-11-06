//! Two-button gesture recognizer (docs/spec/ui.md, "Interaction model").
//!
//! Fed with raw press/release edges and a millisecond clock, it emits gestures:
//! - `Click`: press shorter than 600 ms, not followed by a second press within 300 ms;
//! - `Double`: two short presses within 300 ms;
//! - `Long`: released after 600 ms (and, for A, before auto-repeat starts at 1 s);
//! - `Held`: while a button stays down past 1 s, one tick every 250 ms (lists scroll on it,
//!   hold-to-confirm dialogs count it down).
//!
//! Pure and `no_std`: the same code runs on the device, in the emulator and in tests.

/// One of the two buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Bottom button (GPIO21): next, previous, open.
    A,
    /// Top button (GPIO14): back, home, light.
    B,
}

/// A recognized gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Click(Button),
    Double(Button),
    Long(Button),
    /// Still held; `ms` since the press.
    Held(Button, u32),
}

pub const DEBOUNCE_MS: u64 = 30;
pub const DOUBLE_MS: u64 = 300;
pub const LONG_MS: u64 = 600;
pub const HOLD_MS: u64 = 1000;
pub const HOLD_TICK_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    /// Down since `at`; `second` if this press follows a short one; `ticks` held ticks sent.
    Down {
        at: u64,
        second: bool,
        ticks: u32,
    },
    /// Released after a short press; waiting for a possible second press until `until`.
    WaitDouble {
        until: u64,
    },
}

#[derive(Debug, Clone, Copy)]
struct Track {
    state: State,
    last_edge: u64,
    pressed: bool,
}

impl Track {
    const fn new() -> Self {
        Self {
            state: State::Idle,
            last_edge: 0,
            pressed: false,
        }
    }
}

/// Recognizer for both buttons.
#[derive(Debug, Clone)]
pub struct Gestures {
    a: Track,
    b: Track,
    out: heapless::Deque<Gesture, 8>,
}

impl Default for Gestures {
    fn default() -> Self {
        Self::new()
    }
}

impl Gestures {
    pub const fn new() -> Self {
        Self {
            a: Track::new(),
            b: Track::new(),
            out: heapless::Deque::new(),
        }
    }

    fn track(&mut self, b: Button) -> &mut Track {
        match b {
            Button::A => &mut self.a,
            Button::B => &mut self.b,
        }
    }

    fn emit(&mut self, g: Gesture) {
        if self.out.is_full() {
            let _ = self.out.pop_front();
        }
        let _ = self.out.push_back(g);
    }

    /// A raw edge: `pressed` is the new level of `button` at `now_ms`. Bounces closer than
    /// 30 ms to the previous edge are ignored.
    pub fn edge(&mut self, button: Button, pressed: bool, now_ms: u64) {
        let t = self.track(button);
        if pressed == t.pressed
            || now_ms.saturating_sub(t.last_edge) < DEBOUNCE_MS && t.last_edge != 0
        {
            return;
        }
        t.pressed = pressed;
        t.last_edge = now_ms;
        let state = t.state;
        let mut emit = None;
        t.state = match (pressed, state) {
            (true, State::WaitDouble { until }) if now_ms <= until => State::Down {
                at: now_ms,
                second: true,
                ticks: 0,
            },
            (true, _) => State::Down {
                at: now_ms,
                second: false,
                ticks: 0,
            },
            (false, State::Down { at, second, ticks }) => {
                let held = now_ms.saturating_sub(at);
                if held < LONG_MS {
                    if second {
                        emit = Some(Gesture::Double(button));
                        State::Idle
                    } else {
                        State::WaitDouble {
                            until: now_ms + DOUBLE_MS,
                        }
                    }
                } else {
                    // A auto-repeats once held past 1 s: the release then means nothing more.
                    if button == Button::B || ticks == 0 {
                        emit = Some(Gesture::Long(button));
                    }
                    State::Idle
                }
            }
            (false, s) => s,
        };
        if let Some(g) = emit {
            self.emit(g);
        }
    }

    /// Timed gestures (click after the double window, held ticks). Call often.
    pub fn poll(&mut self, now_ms: u64) -> Option<Gesture> {
        for button in [Button::A, Button::B] {
            let t = self.track(button);
            match t.state {
                State::WaitDouble { until } if now_ms > until => {
                    t.state = State::Idle;
                    self.emit(Gesture::Click(button));
                }
                State::Down { at, second, ticks } => {
                    let held = now_ms.saturating_sub(at);
                    if held >= HOLD_MS {
                        let due = ((held - HOLD_MS) / HOLD_TICK_MS) as u32 + 1;
                        if due > ticks {
                            t.state = State::Down {
                                at,
                                second,
                                ticks: due,
                            };
                            self.emit(Gesture::Held(button, held as u32));
                        }
                    }
                }
                _ => {}
            }
        }
        self.out.pop_front()
    }

    /// When the next timed gesture may fire, to sleep until then.
    pub fn next_deadline(&self) -> Option<u64> {
        let d = |t: &Track| match t.state {
            State::WaitDouble { until } => Some(until + 1),
            State::Down { at, ticks, .. } => Some(at + HOLD_MS + u64::from(ticks) * HOLD_TICK_MS),
            State::Idle => None,
        };
        match (d(&self.a), d(&self.b)) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    fn run(events: &[(Button, bool, u64)], until: u64) -> Vec<Gesture> {
        let mut g = Gestures::new();
        let mut out = Vec::new();
        let mut i = 0;
        for now in 0..=until {
            while i < events.len() && events[i].2 == now {
                g.edge(events[i].0, events[i].1, now);
                i += 1;
            }
            while let Some(x) = g.poll(now) {
                out.push(x);
            }
        }
        out
    }

    use Button::*;
    use Gesture::*;

    #[test]
    fn click_after_double_window() {
        assert_eq!(run(&[(A, true, 100), (A, false, 200)], 600), vec![Click(A)]);
    }

    #[test]
    fn double_click() {
        assert_eq!(
            run(
                &[
                    (A, true, 100),
                    (A, false, 180),
                    (A, true, 300),
                    (A, false, 380)
                ],
                900
            ),
            vec![Double(A)]
        );
    }

    #[test]
    fn long_press_on_release() {
        assert_eq!(run(&[(B, true, 100), (B, false, 800)], 1200), vec![Long(B)]);
        assert_eq!(run(&[(A, true, 100), (A, false, 900)], 1200), vec![Long(A)]);
    }

    #[test]
    fn a_held_repeats_and_does_not_open() {
        let g = run(&[(A, true, 100), (A, false, 1700)], 2000);
        assert_eq!(g, vec![Held(A, 1000), Held(A, 1250), Held(A, 1500)]);
    }

    #[test]
    fn b_held_still_toggles_light() {
        let g = run(&[(B, true, 100), (B, false, 1600)], 2000);
        assert_eq!(g.last(), Some(&Long(B)));
    }

    #[test]
    fn bounces_are_ignored() {
        let g = run(
            &[
                (A, true, 100),
                (A, false, 110),
                (A, true, 115),
                (A, false, 220),
            ],
            700,
        );
        assert_eq!(g, vec![Click(A)]);
    }

    #[test]
    fn buttons_are_independent() {
        let g = run(
            &[
                (A, true, 100),
                (B, true, 120),
                (A, false, 200),
                (B, false, 230),
            ],
            800,
        );
        assert_eq!(g, vec![Click(A), Click(B)]);
    }
}
