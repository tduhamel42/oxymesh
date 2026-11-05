//! Émission : file à priorités, budget de temps d'antenne, écoute avant émission
//! (`Dispatcher.cpp`, `StaticPoolPacketManager.cpp` ; `docs/spec/air.md` §4.5).

use mc_air::PacketBuf;
use mc_core::{Bandwidth, CodingRate, Duration, DutyBudget, FRAME_MAX, Instant, LoRa};
use rand_core::Rng;

/// Trames en attente d'émission (le C++ a un pool de 16 paquets, `MyMesh.cpp:901`).
pub const POOL: usize = 16;
/// Fenêtre du budget de temps d'antenne du C++ (`getDutyCycleWindowMs`).
const WINDOW_MS: u64 = 3_600_000;
/// Budget minimal gardé en réserve (`MIN_TX_BUDGET_RESERVE_MS`).
const RESERVE_MS: u64 = 100;
/// Canal occupé plus longtemps que ça : on émet quand même (`getCADFailMaxDuration`).
const BUSY_MAX: Duration = Duration::from_millis(4000);

/// Réglage radio courant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioParams {
    pub freq_khz: u32,
    pub bw_hz: u32,
    pub sf: u8,
    /// 5 à 8 (4/5 à 4/8).
    pub cr: u8,
    pub tx_power_dbm: i8,
}

impl RadioParams {
    /// Modulation équivalente, avec le préambule de MeshCore : 32 symboles si SF ≤ 8, sinon 16
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

    /// Temps d'antenne estimé en millisecondes entières, comme `getEstAirtimeFor`
    /// (RadioLib, microsecondes divisées par 1000).
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

/// Trame à émettre maintenant.
#[derive(Debug, Clone)]
pub struct TxFrame {
    pub raw: heapless::Vec<u8, FRAME_MAX>,
    pub airtime: Duration,
    pub flood: bool,
}

#[derive(Debug, Clone)]
pub struct Dispatcher {
    queue: heapless::Vec<Outbound, POOL>,
    /// Budget du C++ : `1 / (1 + airtime_factor)` de la fenêtre, en ms.
    budget_ms: u64,
    last_budget: Instant,
    next_tx: Instant,
    busy_since: Option<Instant>,
    in_flight: Option<(Instant, Instant, bool)>,
    /// Borne réglementaire de la sous-bande (config/eu868.toml), tenue en plus.
    pub regulatory: Option<DutyBudget>,
    pub stats: Stats,
}

/// Compteurs (`GET_STATS`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub sent_flood: u32,
    pub sent_direct: u32,
    pub recv_flood: u32,
    pub recv_direct: u32,
    pub recv_errors: u32,
    pub tx_airtime_ms: u64,
    pub rx_airtime_ms: u64,
    /// `ERR_EVENT_*` du C++ : 1 pool plein, 2 écoute avant émission trop longue.
    pub err_flags: u16,
    pub dropped_full: u32,
}

pub const ERR_EVENT_FULL: u16 = 1 << 0;
pub const ERR_EVENT_CAD_TIMEOUT: u16 = 1 << 1;

/// `airtime_factor` × 1000 (le C++ le garde en `float`, défaut 1,0).
fn duty_div(af_milli: u32) -> u64 {
    1000 + u64::from(af_milli)
}

impl Dispatcher {
    pub fn new(now: Instant, af_milli: u32) -> Self {
        Self {
            queue: heapless::Vec::new(),
            budget_ms: WINDOW_MS * 1000 / duty_div(af_milli),
            last_budget: now,
            next_tx: now,
            busy_since: None,
            in_flight: None,
            regulatory: None,
            stats: Stats::default(),
        }
    }

    /// Met une trame en file. Faux si la file est pleine (le C++ perd la trame : pool vide).
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

    /// Trames en file, pour l'inspection (tests, statistiques).
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

    /// Prochain instant où quelque chose peut changer côté émission.
    pub fn next_wakeup(&self) -> Option<Instant> {
        if let Some((_, expiry, _)) = self.in_flight {
            return Some(expiry);
        }
        self.queue.iter().map(|o| o.due.max(self.next_tx)).min()
    }

    /// Choisit la trame à émettre maintenant (`checkSend`). `busy` : la radio est en train de
    /// recevoir (préambule ou en-tête détecté), c'est l'écoute avant émission.
    pub fn poll_tx(
        &mut self,
        now: Instant,
        busy: bool,
        radio: &RadioParams,
        af_milli: u32,
        rng: &mut impl Rng,
    ) -> Option<TxFrame> {
        if let Some((_, expiry, _)) = self.in_flight {
            if now < expiry {
                return None;
            }
            // Émission jamais terminée : abandonnée (`outbound_expiry`).
            self.in_flight = None;
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
        if busy {
            let since = *self.busy_since.get_or_insert(now);
            if now.saturating_duration_since(since) > BUSY_MAX {
                self.stats.err_flags |= ERR_EVENT_CAD_TIMEOUT;
            } else {
                // `Mesh::getCADFailRetryDelay` : 120, 240 ou 360 ms.
                let k = 1 + u64::from(rng.next_u32() % 3);
                self.next_tx = now + Duration::from_millis(120 * k);
                return None;
            }
        }
        self.busy_since = None;
        // La plus petite priorité parmi les trames dues ; à égalité, la plus ancienne.
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
                // Hors budget réglementaire : la trame attend son heure.
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

    /// Fin d'émission (`isSendComplete`) : décompte le budget et fixe la prochaine émission.
    pub fn tx_done(&mut self, now: Instant, af_milli: u32) {
        let Some((start, _, flood)) = self.in_flight.take() else {
            return;
        };
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

/// `nextInt(a, b)` du C++ : `rand32 % (b − a) + a` (`Utils.cpp:18-22`).
pub fn next_int(rng: &mut impl Rng, a: u32, b: u32) -> u32 {
    if b <= a {
        return a;
    }
    rng.next_u32() % (b - a) + a
}
