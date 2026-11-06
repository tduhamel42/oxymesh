//! When to refresh the e-paper fully rather than partially (`docs/spec/ui.md` §10).
//!
//! Partial refreshes leave ghosts that build up; a full refresh clears them but flashes the
//! panel for 2 s. The policy keeps a debt: each partial refresh adds to it, a full refresh
//! clears it, and the full refresh is placed where it bothers least (after input stops, or on
//! a screen change).

/// What to send to the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Partial,
    Full,
}

/// Debt added by a partial refresh, and once the debt is high.
const PARTIAL: u32 = 100;
const PARTIAL_IN_DEBT: u32 = 150;
/// From here, a full refresh runs once input has stopped for `IDLE_MS`.
pub const IN_DEBT: u32 = 1_000;
/// From here, the next screen change is a full refresh.
pub const HEAVY: u32 = 3_000;
pub const IDLE_MS: u64 = 60_000;

#[derive(Debug, Clone, Default)]
pub struct Policy {
    debt: u32,
    last_input_ms: u64,
    last_hash: Option<u64>,
    sent: bool,
}

/// FNV-1a over the panel bytes: equal frames are not sent again.
pub fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

impl Policy {
    pub const fn new() -> Self {
        Self {
            debt: 0,
            last_input_ms: 0,
            last_hash: None,
            sent: false,
        }
    }

    pub fn debt(&self) -> u32 {
        self.debt
    }

    /// A button was pressed.
    pub fn input(&mut self, now_ms: u64) {
        self.last_input_ms = now_ms;
    }

    /// Something was sent (message, advert): the next screen change cleans the panel.
    pub fn sent(&mut self) {
        self.sent = true;
    }

    /// A new frame is ready: how to show it, or `None` if it is identical to the last one.
    pub fn frame(&mut self, hash: u64, screen_changed: bool) -> Option<Kind> {
        let first = self.last_hash.is_none();
        if self.last_hash == Some(hash) {
            return None;
        }
        self.last_hash = Some(hash);
        let full = first || (screen_changed && (self.sent || self.debt >= HEAVY));
        Some(self.account(full))
    }

    /// Nothing new to show: a full refresh of the same frame if the debt calls for it.
    pub fn idle(&mut self, now_ms: u64) -> Option<Kind> {
        if self.last_hash.is_some()
            && self.debt >= IN_DEBT
            && now_ms.saturating_sub(self.last_input_ms) >= IDLE_MS
        {
            Some(self.account(true))
        } else {
            None
        }
    }

    fn account(&mut self, full: bool) -> Kind {
        if full {
            self.debt = 0;
            self.sent = false;
            Kind::Full
        } else {
            self.debt += if self.debt >= IN_DEBT {
                PARTIAL_IN_DEBT
            } else {
                PARTIAL
            };
            Kind::Partial
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_frame_is_full_then_partial_and_unchanged_frames_are_skipped() {
        let mut p = Policy::new();
        assert_eq!(p.frame(1, false), Some(Kind::Full));
        assert_eq!(p.frame(2, false), Some(Kind::Partial));
        assert_eq!(p.frame(2, false), None);
        assert_eq!(p.debt(), 100);
    }

    #[test]
    fn debt_grows_faster_once_high() {
        let mut p = Policy::new();
        p.frame(0, false);
        for h in 1..=10 {
            p.frame(h, false);
        }
        assert_eq!(p.debt(), 1_000);
        p.frame(11, false);
        assert_eq!(p.debt(), 1_150);
    }

    #[test]
    fn full_refresh_after_input_stops_once_in_debt() {
        let mut p = Policy::new();
        p.frame(0, false);
        for h in 1..=9 {
            p.input(h * 1000);
            p.frame(h, false);
        }
        assert_eq!(p.idle(100_000), None, "debt 900: no full refresh");
        p.input(10_000);
        p.frame(10, false);
        assert_eq!(p.idle(69_999), None);
        assert_eq!(p.idle(70_000), Some(Kind::Full));
        assert_eq!(p.debt(), 0);
        assert_eq!(p.idle(200_000), None);
    }

    #[test]
    fn screen_change_is_full_after_a_send_or_heavy_use() {
        let mut p = Policy::new();
        p.frame(0, false);
        p.sent();
        assert_eq!(p.frame(1, false), Some(Kind::Partial), "same screen");
        assert_eq!(p.frame(2, true), Some(Kind::Full));
        assert_eq!(p.frame(3, true), Some(Kind::Partial));
        for h in 4..30 {
            p.frame(h, false);
        }
        assert!(p.debt() >= HEAVY);
        assert_eq!(p.frame(100, true), Some(Kind::Full));
    }

    #[test]
    fn hash_tells_frames_apart() {
        assert_ne!(hash(&[0, 1]), hash(&[1, 0]));
        assert_eq!(hash(&[7; 5000]), hash(&[7; 5000]));
    }
}
