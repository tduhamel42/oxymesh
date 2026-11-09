//! Transmission: priority queue, airtime budget, listen before talk
//! (`Dispatcher.cpp`, `StaticPoolPacketManager.cpp`; `docs/spec/air.md` §4.5).

use mc_air::PacketBuf;
use mc_core::{Bandwidth, CodingRate, Duration, DutyBudget, FRAME_MAX, Instant, LoRa};
use rand_core::Rng;

/// Frames waiting to be transmitted (the C++ has a pool of 16 packets, `MyMesh.cpp:901`).
pub const POOL: usize = 16;
/// Window of the C++ airtime budget (`getDutyCycleWindowMs`).
const WINDOW_MS: u64 = 3_600_000;
/// Minimum budget kept in reserve (`MIN_TX_BUDGET_RESERVE_MS`).
const RESERVE_MS: u64 = 100;
/// Channel busy for longer than this: transmit anyway (`getCADFailMaxDuration`).
const BUSY_MAX: Duration = Duration::from_millis(4000);

/// Current radio settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioParams {
    pub freq_khz: u32,
    pub bw_hz: u32,
    pub sf: u8,
    /// 5 to 8 (4/5 to 4/8).
    pub cr: u8,
    pub tx_power_dbm: i8,
}

impl RadioParams {
    /// Equivalent modulation, with the MeshCore preamble: 32 symbols if SF ≤ 8, otherwise 16
    /// (`RadioLibWrappers.h:56`).
    pub fn lora(&self) -> LoRa {
        let bw = Bandwidth::from_hz(self.bw_hz).unwrap_or(Bandwidth::Bw62_5);
        let mut l = LoRa::new(self.sf, bw);
        l.cr = match self.cr {
            6 => CodingRate::Cr4_6,
            7 => CodingRate::Cr4_7,
            8 => CodingRate::Cr4_8,
            _ => CodingRate::Cr4_5,
        };
        l.preamble = if self.sf <= 8 { 32 } else { 16 };
        l
    }

    /// Estimated airtime in whole milliseconds, like `getEstAirtimeFor`
    /// (RadioLib, microseconds divided by 1000).
    pub fn airtime_ms(&self, len: usize) -> u64 {
        self.lora().airtime(len).as_micros() / 1000
    }
}

#[derive(Debug, Clone)]
pub struct Outbound {
    pub pkt: PacketBuf,
    pub priority: u8,
    pub due: Instant,
}

/// Frame to transmit now.
#[derive(Debug, Clone)]
pub struct TxFrame {
    pub raw: heapless::Vec<u8, FRAME_MAX>,
    pub airtime: Duration,
    pub flood: bool,
}

#[derive(Debug, Clone)]
pub struct Dispatcher {
    queue: heapless::Vec<Outbound, POOL>,
    /// C++ budget: `1 / (1 + airtime_factor)` of the window, in ms.
    budget_ms: u64,
    last_budget: Instant,
    next_tx: Instant,
    busy_since: Option<Instant>,
    in_flight: Option<(Instant, Instant, bool)>,
    /// Next AGC reset (`next_agc_reset_time`); a transmission that just ended re-arms it.
    next_agc: Instant,
    tx_ended: Option<Instant>,
    /// Regulatory limit of the sub-band (config/eu868.toml), enforced on top.
    pub regulatory: Option<DutyBudget>,
    pub stats: Stats,
}

/// Counters (`GET_STATS`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub sent_flood: u32,
    pub sent_direct: u32,
    pub recv_flood: u32,
    pub recv_direct: u32,
    pub recv_errors: u32,
    pub tx_airtime_ms: u64,
    pub rx_airtime_ms: u64,
    /// C++ `ERR_EVENT_*`: 1 pool full, 2 listen before talk too long.
    pub err_flags: u16,
    pub dropped_full: u32,
}

pub const ERR_EVENT_FULL: u16 = 1 << 0;
pub const ERR_EVENT_CAD_TIMEOUT: u16 = 1 << 1;

/// `airtime_factor` × 1000 (the C++ keeps it as a `float`, default 1.0).
const fn duty_div(af_milli: u32) -> u64 {
    1000 + af_milli as u64
}

impl Stats {
    pub const ZERO: Self = Self {
        sent_flood: 0,
        sent_direct: 0,
        recv_flood: 0,
        recv_direct: 0,
        recv_errors: 0,
        tx_airtime_ms: 0,
        rx_airtime_ms: 0,
        err_flags: 0,
        dropped_full: 0,
    };
}

impl Dispatcher {
    pub const fn new(now: Instant, af_milli: u32) -> Self {
        Self {
            queue: heapless::Vec::new(),
            budget_ms: WINDOW_MS * 1000 / duty_div(af_milli),
            last_budget: now,
            next_tx: now,
            busy_since: None,
            in_flight: None,
            next_agc: now,
            tx_ended: None,
            regulatory: None,
            stats: Stats::ZERO,
        }
    }

    /// Queues a frame. False if the queue is full (the C++ drops the frame: pool empty).
    pub fn queue(&mut self, pkt: PacketBuf, priority: u8, due: Instant) -> bool {
        if self.queue.push(Outbound { pkt, priority, due }).is_err() {
            self.stats.err_flags |= ERR_EVENT_FULL;
            self.stats.dropped_full = self.stats.dropped_full.wrapping_add(1);
            return false;
        }
        true
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Queued frames, for inspection (tests, statistics).
    pub fn queued(&self) -> impl Iterator<Item = &Outbound> {
        self.queue.iter()
    }

    fn refill(&mut self, now: Instant, af_milli: u32) {
        let elapsed = now.saturating_duration_since(self.last_budget).as_millis();
        let max = WINDOW_MS * 1000 / duty_div(af_milli);
        let refill = elapsed * 1000 / duty_div(af_milli);
        if refill > 0 {
            self.budget_ms = (self.budget_ms + refill).min(max);
            self.last_budget = now;
        }
    }

    /// Is an AGC reset due (`Dispatcher::loop`, `Dispatcher.cpp:127-136`)? `interval_ms` 0 = never.
    /// The first one is due at once; each transmission pushes the next one `interval_ms` after
    /// it ends; none while transmitting. When this says yes, the next one is already scheduled,
    /// whether or not the radio could do it (the C++ skips it while a packet is coming in).
    pub fn agc_due(&mut self, now: Instant, interval_ms: u64) -> bool {
        if self.in_flight.is_some() {
            return false;
        }
        let interval = Duration::from_millis(interval_ms);
        if let Some(t) = self.tx_ended.take() {
            self.next_agc = t + interval;
        }
        if interval_ms == 0 || now < self.next_agc {
            return false;
        }
        self.next_agc = now + interval;
        true
    }

    /// Next instant when something may change on the transmit side.
    pub fn next_wakeup(&self) -> Option<Instant> {
        if let Some((_, expiry, _)) = self.in_flight {
            return Some(expiry);
        }
        self.queue.iter().map(|o| o.due.max(self.next_tx)).min()
    }

    /// Picks the frame to transmit now (`checkSend`). `busy` is listen before talk: is a packet
    /// coming in, or the channel otherwise in use (`isReceiving`)? Like the C++, it is only asked
    /// when a frame is ready to go.
    pub fn poll_tx(
        &mut self,
        now: Instant,
        busy: impl FnOnce() -> bool,
        radio: &RadioParams,
        af_milli: u32,
        rng: &mut impl Rng,
    ) -> Option<TxFrame> {
        if let Some((_, expiry, _)) = self.in_flight {
            if now < expiry {
                return None;
            }
            // Transmission never completed: abandoned (`outbound_expiry`).
            self.in_flight = None;
            self.tx_ended = Some(now);
        }
        if !self.queue.iter().any(|o| o.due <= now) {
            return None;
        }
        self.refill(now, af_milli);
        let est = radio.airtime_ms(FRAME_MAX);
        if self.budget_ms < est / 2 {
            let needed = est / 2 - self.budget_ms;
            self.next_tx = now + Duration::from_millis(needed * duty_div(af_milli) / 1000);
            return None;
        }
        if now < self.next_tx {
            return None;
        }
        if busy() {
            let since = *self.busy_since.get_or_insert(now);
            if now.saturating_duration_since(since) > BUSY_MAX {
                self.stats.err_flags |= ERR_EVENT_CAD_TIMEOUT;
            } else {
                // `Mesh::getCADFailRetryDelay`: 120, 240 or 360 ms.
                let k = 1 + u64::from(rng.next_u32() % 3);
                self.next_tx = now + Duration::from_millis(120 * k);
                return None;
            }
        }
        self.busy_since = None;
        // The lowest priority among the due frames; on a tie, the oldest.
        let mut best: Option<(usize, u8)> = None;
        for (i, o) in self.queue.iter().enumerate() {
            if o.due <= now && best.is_none_or(|(_, p)| o.priority < p) {
                best = Some((i, o.priority));
            }
        }
        let (i, _) = best?;
        let o = self.queue.remove(i);
        let mut raw = [0u8; FRAME_MAX];
        let n = o.pkt.encode(&mut raw).ok()?;
        let airtime = radio.lora().airtime(n);
        if let Some(reg) = self.regulatory.as_mut() {
            if !reg.can_transmit(now, airtime) {
                // Over the regulatory budget: the frame waits for its turn.
                let at = reg.clone().next_opportunity(now, airtime);
                if let Some(at) = at {
                    self.next_tx = at;
                    let _ = self.queue.insert(i, o);
                }
                return None;
            }
            reg.record(now, airtime);
        }
        let flood = o.pkt.header.route.is_flood();
        let expiry = now + Duration::from_millis(radio.airtime_ms(n) * 3 / 2 + 1);
        self.in_flight = Some((now, expiry, flood));
        let mut v = heapless::Vec::new();
        v.extend_from_slice(raw.get(..n).unwrap_or(&[])).ok()?;
        Some(TxFrame {
            raw: v,
            airtime,
            flood,
        })
    }

    /// End of transmission (`isSendComplete`): charges the budget and sets the next transmission.
    pub fn tx_done(&mut self, now: Instant, af_milli: u32) {
        let Some((start, _, flood)) = self.in_flight.take() else {
            return;
        };
        self.tx_ended = Some(now);
        let t = now.saturating_duration_since(start).as_millis();
        self.stats.tx_airtime_ms += t;
        self.refill(now, af_milli);
        self.budget_ms = self.budget_ms.saturating_sub(t);
        if self.budget_ms < RESERVE_MS {
            let needed = RESERVE_MS - self.budget_ms;
            self.next_tx = now + Duration::from_millis(needed * duty_div(af_milli) / 1000);
        } else {
            self.next_tx = now;
        }
        if flood {
            self.stats.sent_flood = self.stats.sent_flood.wrapping_add(1);
        } else {
            self.stats.sent_direct = self.stats.sent_direct.wrapping_add(1);
        }
    }

    pub fn is_transmitting(&self) -> bool {
        self.in_flight.is_some()
    }
}

/// The C++ `nextInt(a, b)`: `rand32 % (b − a) + a` (`Utils.cpp:18-22`).
pub fn next_int(rng: &mut impl Rng, a: u32, b: u32) -> u32 {
    if b <= a {
        return a;
    }
    rng.next_u32() % (b - a) + a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        Instant::from_nanos(ms * 1_000_000)
    }

    #[test]
    fn agc_reset_schedule() {
        let mut d = Dispatcher::new(at(0), 1000);
        // Off: never.
        assert!(!d.agc_due(at(0), 0));
        assert!(!d.agc_due(at(100_000), 0));
        // On: due at once, then every interval.
        assert!(d.agc_due(at(100_000), 60_000));
        assert!(!d.agc_due(at(159_999), 60_000));
        assert!(d.agc_due(at(160_000), 60_000));
        // A transmission: nothing while it is in the air, then the interval counts from its end.
        d.in_flight = Some((at(200_000), at(210_000), true));
        assert!(!d.agc_due(at(220_001), 60_000));
        d.tx_done(at(230_000), 1000);
        assert!(!d.agc_due(at(280_000), 60_000));
        assert!(d.agc_due(at(290_000), 60_000));
    }
}
