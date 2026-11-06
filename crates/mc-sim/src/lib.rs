//! Simulated radio: devices (`mc-node` with their companion layer) connected by links, half
//! duplex, with collisions and listen-before-talk. Deterministic: a single seed.
//!
//! The model is deliberately simple: a frame is received by every neighbour that neither
//! transmits nor receives anything else during its whole duration; an overlap destroys both
//! frames.

use std::collections::BTreeMap;

use mc_air::crypto::LocalIdentity;
use mc_core::{Duration, Instant};
use mc_node::device::{Frame, Info};
use mc_node::persist::{CHANNEL_RECORD, CONTACT_RECORD, PREFS_RECORD};
use mc_node::{Device, Event, Node, Prefs, Storage};
use rand_chacha::ChaCha20Rng;
use rand_core::{Rng, SeedableRng};

/// In-memory storage (tests, virtual device).
#[derive(Debug, Default, Clone)]
pub struct MemStorage {
    pub identity: Option<([u8; 64], [u8; 32])>,
    pub prefs: Option<Vec<u8>>,
    pub contacts: Vec<Vec<u8>>,
    pub channels: BTreeMap<u8, Vec<u8>>,
    pub blobs: BTreeMap<[u8; 32], Vec<u8>>,
}

impl Storage for MemStorage {
    fn save_identity(&mut self, prv: &[u8; 64], pub_key: &[u8; 32]) {
        self.identity = Some((*prv, *pub_key));
    }
    fn save_prefs(&mut self, rec: &[u8; PREFS_RECORD]) {
        self.prefs = Some(rec.to_vec());
    }
    fn save_contacts(&mut self, records: &mut dyn Iterator<Item = [u8; CONTACT_RECORD]>) {
        self.contacts = records.map(|r| r.to_vec()).collect();
    }
    fn save_channel(&mut self, idx: u8, rec: &[u8; CHANNEL_RECORD]) {
        self.channels.insert(idx, rec.to_vec());
    }
    fn put_blob(&mut self, key: &[u8; 32], raw: &[u8]) {
        self.blobs.insert(*key, raw.to_vec());
    }
    fn get_blob(&mut self, key: &[u8; 32], out: &mut [u8]) -> Option<usize> {
        let b = self.blobs.get(key)?;
        out.get_mut(..b.len())?.copy_from_slice(b);
        Some(b.len())
    }
    fn remove_blob(&mut self, key: &[u8; 32]) {
        self.blobs.remove(key);
    }
    fn usage_kb(&self) -> (u32, u32) {
        let used =
            self.blobs.values().map(Vec::len).sum::<usize>() + self.contacts.len() * CONTACT_RECORD;
        ((used / 1024) as u32, 768)
    }
}

/// What the virtual device reports.
pub const SIM_INFO: Info = Info {
    manufacturer: "oxymesh sim",
    version: "v1.18.0-oxymesh",
    build_date: "06-Nov-2026",
    max_tx_power: 22,
    has_gps: false,
};

#[allow(missing_debug_implementations)]
pub struct SimNode {
    pub dev: Device,
    pub st: MemStorage,
    rng: ChaCha20Rng,
    /// End of the transmission in progress.
    tx_until: Option<Instant>,
    /// Produced events (copies meant for the UI), kept for the tests.
    pub events: Vec<Event>,
    /// Frames sent to the app outside direct responses (notifications, contacts).
    pub app: Vec<Frame>,
}

struct OnAir {
    from: usize,
    raw: Vec<u8>,
    start: Instant,
    end: Instant,
}

/// A transmitted frame: (instant, sender, bytes).
pub type Heard = (Instant, usize, Vec<u8>);

#[allow(missing_debug_implementations)]
pub struct World {
    pub nodes: Vec<SimNode>,
    /// `links[a][b]`: b hears a, with this SNR (×4).
    links: Vec<Vec<Option<i8>>>,
    pub now: Instant,
    on_air: Vec<OnAir>,
    /// Random losses, in thousandths.
    pub loss_milli: u32,
    rng: ChaCha20Rng,
    /// Log of all transmissions.
    pub log: Vec<Heard>,
}

/// Deterministic test identity.
pub fn identity(label: &str) -> LocalIdentity {
    let mut i = 0u32;
    loop {
        let seed: [u8; 32] = mc_air::crypto::sha256_trunc(&[label.as_bytes(), &i.to_le_bytes()]);
        let id = LocalIdentity::from_seed(&seed);
        if !id.is_reserved() {
            return id;
        }
        i += 1;
    }
}

/// Start epoch of the simulations.
pub const EPOCH: u32 = 1_793_900_000;

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            nodes: Vec::new(),
            links: Vec::new(),
            now: Instant::ZERO,
            on_air: Vec::new(),
            loss_milli: 0,
            rng: ChaCha20Rng::seed_from_u64(seed),
            log: Vec::new(),
        }
    }

    /// Adds a device named `name` (connected to an app); returns its index.
    pub fn add(&mut self, name: &str, tweak: impl FnOnce(&mut Prefs)) -> usize {
        let id = identity(name);
        let mut prefs = Prefs::defaults(id.pub_key());
        prefs.name = [0; 32];
        let n = name.len().min(31);
        prefs.name[..n].copy_from_slice(&name.as_bytes()[..n]);
        tweak(&mut prefs);
        let node = Node::new(id, prefs, EPOCH, self.now);
        let seed = self.rng.next_u64();
        let mut dev = Device::new(node, SIM_INFO);
        dev.connected = true;
        self.nodes.push(SimNode {
            dev,
            st: MemStorage::default(),
            rng: ChaCha20Rng::seed_from_u64(seed),
            tx_until: None,
            events: Vec::new(),
            app: Vec::new(),
        });
        for row in &mut self.links {
            row.push(None);
        }
        self.links.push(vec![None; self.nodes.len()]);
        self.nodes.len() - 1
    }

    /// Link in both directions, SNR 10 dB.
    pub fn link(&mut self, a: usize, b: usize) {
        self.links[a][b] = Some(40);
        self.links[b][a] = Some(40);
    }

    pub fn unlink(&mut self, a: usize, b: usize) {
        self.links[a][b] = None;
        self.links[b][a] = None;
    }

    fn hears(&self, from: usize, to: usize) -> Option<i8> {
        self.links
            .get(from)
            .and_then(|r| r.get(to))
            .copied()
            .flatten()
    }

    /// Is the channel busy for `n` (a neighbour is transmitting)?
    fn busy(&self, n: usize) -> bool {
        self.on_air
            .iter()
            .any(|t| t.from != n && self.hears(t.from, n).is_some() && t.start <= self.now)
    }

    /// Advances by one step of `dt`.
    pub fn step(&mut self, dt: Duration) {
        self.now = self.now + dt;
        let now = self.now;
        // Transmission ends: delivery to neighbours.
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.on_air.len() {
            if self.on_air[i].end <= now {
                done.push(self.on_air.remove(i));
            } else {
                i += 1;
            }
        }
        for t in &done {
            if let Some(n) = self.nodes.get_mut(t.from) {
                n.dev.tx_done(t.end);
                n.tx_until = None;
            }
        }
        for t in &done {
            for to in 0..self.nodes.len() {
                let Some(snr4) = self.hears(t.from, to) else {
                    continue;
                };
                if to == t.from {
                    continue;
                }
                // Half duplex: a node that transmitted during the frame does not hear it.
                let deaf = self.log_overlaps_tx(to, t.start, t.end);
                // Collision: another audible frame overlaps this one.
                let collided = done.iter().chain(self.on_air.iter()).any(|o| {
                    !std::ptr::eq(o, t)
                        && o.from != to
                        && self.hears(o.from, to).is_some()
                        && o.start < t.end
                        && t.start < o.end
                });
                let lost = self.loss_milli > 0 && self.rng.next_u32() % 1000 < self.loss_milli;
                if deaf || collided || lost {
                    continue;
                }
                let sn = &mut self.nodes[to];
                sn.dev
                    .on_rx(now, &t.raw, snr4, -90, &mut sn.rng, &mut sn.st);
            }
        }
        // Transmissions.
        for n in 0..self.nodes.len() {
            if self.nodes[n].tx_until.is_some() {
                continue;
            }
            let busy = self.busy(n);
            let sn = &mut self.nodes[n];
            if let Some(f) = sn.dev.poll_tx(now, busy, &mut sn.rng) {
                let end = now + f.airtime;
                sn.tx_until = Some(end);
                self.log.push((now, n, f.raw.to_vec()));
                self.on_air.push(OnAir {
                    from: n,
                    raw: f.raw.to_vec(),
                    start: now,
                    end,
                });
            }
        }
        for sn in &mut self.nodes {
            sn.dev.tick(now, &mut sn.rng, &mut sn.st);
            while let Some(e) = sn.dev.poll_ui() {
                sn.events.push(e);
            }
            while let Some(f) = sn.dev.poll_app() {
                sn.app.push(f);
            }
        }
    }

    fn log_overlaps_tx(&self, n: usize, start: Instant, end: Instant) -> bool {
        self.log.iter().any(|(t0, from, raw)| {
            if *from != n {
                return false;
            }
            let t1 = *t0 + self.nodes[n].dev.node.prefs.radio.lora().airtime(raw.len());
            *t0 < end && start < t1
        })
    }

    /// Advances until `pred` is true, for at most `max`. True if reached.
    pub fn run_until(&mut self, max: Duration, mut pred: impl FnMut(&World) -> bool) -> bool {
        let end = self.now + max;
        while self.now < end {
            if pred(self) {
                return true;
            }
            self.step(Duration::from_millis(5));
        }
        pred(self)
    }

    /// Advances by `d`.
    pub fn run(&mut self, d: Duration) {
        let end = self.now + d;
        while self.now < end {
            self.step(Duration::from_millis(5));
        }
    }

    pub fn node(&mut self, i: usize) -> &mut Node {
        &mut self.nodes[i].dev.node
    }

    /// An app frame to device `i`; returns what the device writes immediately (response,
    /// notifications, contacts of a `GET_CONTACTS`).
    pub fn app(&mut self, i: usize, frame: &[u8]) -> Vec<Frame> {
        let now = self.now;
        let sn = &mut self.nodes[i];
        sn.dev.on_app_frame(now, frame, &mut sn.rng, &mut sn.st);
        let mut out = Vec::new();
        while let Some(f) = sn.dev.poll_app() {
            out.push(f);
        }
        while let Some(e) = sn.dev.poll_ui() {
            sn.events.push(e);
        }
        out
    }

    /// The events of `i`, drained.
    pub fn take_events(&mut self, i: usize) -> Vec<Event> {
        std::mem::take(&mut self.nodes[i].events)
    }
}
