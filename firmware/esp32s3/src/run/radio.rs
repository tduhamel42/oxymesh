//! The SX1262 side of the main loop: received packets to the node, frames from the node to the
//! air after listen before talk, the latched IRQ flags expired like the C++
//! (`mc_core::rx_watch`), the noise floor, and the optional AGC reset (`docs/spec/air.md` §4.5).
//! The board's radio front end (amplifier, LNA) follows through [`FrontEnd`].

use embassy_time::{Duration, Instant};
use log::error;
use mc_core::noise::NoiseFloor;
use mc_core::rx_watch::{self, RxWatch, Watch};
use mc_node::{Device, Prefs, Storage};
use rand_core::Rng;

use crate::sx126x::{Error, LoRaConfig, Sx126x, irq};

/// A board's radio front end, switched around transmissions like the C++'s
/// `onBeforeTransmit` / `onAfterTransmit` (`RadioLibWrappers.cpp:166-192`).
pub trait FrontEnd {
    /// About to transmit.
    fn tx(&mut self) {}
    /// Back to receiving.
    fn rx(&mut self) {}
    /// The radio sleeps (hibernation).
    fn sleep(&mut self) {}
    /// Settings changed (the LNA setting, for one).
    fn apply(&mut self, _prefs: &Prefs) {}
}

/// No front end: the SX1262 drives the antenna itself (ThinkNode M5).
#[derive(Debug, Default)]
pub struct NoFrontEnd;

impl FrontEnd for NoFrontEnd {}

/// How the board wires the SX1262.
#[derive(Debug, Clone, Copy)]
pub struct Chip {
    /// TCXO supply on DIO3.
    pub tcxo_millivolts: u16,
    /// DC-DC regulator (otherwise LDO).
    pub dcdc: bool,
    /// PA current limit (`SX126X_CURRENT_LIMIT`).
    pub ocp_ma: u16,
    /// Set bit 0 of register 0x08B5 (`SX126X_REGISTER_PATCH`, Heltec V4).
    pub rx_patch: bool,
}

/// What happened to a transmission, for the board's TX light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tx {
    Started,
    Done,
}

/// SX1262 settings from the node prefs (preamble 32 if SF ≤ 8, otherwise 16).
pub fn lora_of(p: &Prefs) -> LoRaConfig {
    LoRaConfig {
        freq_hz: p.radio.freq_khz.saturating_mul(1000),
        sf: p.radio.sf,
        bw_hz: p.radio.bw_hz,
        cr: p.radio.cr.saturating_sub(4).clamp(1, 4),
        preamble: if p.radio.sf <= 8 { 32 } else { 16 },
        sync_word: 0x12,
        rx_boosted: p.rx_boosted_gain,
    }
}

/// Listen before talk, when a frame is ready (`RadioLibWrapper::isReceiving`,
/// `RadioLibWrappers.h:44-48` and `.cpp:198-214`): a packet coming in; the channel louder than
/// the noise floor by `int.thresh` (when set); a LoRa preamble found by CAD (when on), after
/// which reception restarts.
fn channel_busy(
    sx: &mut Sx126x<'_>,
    receiving: bool,
    (int_thresh, cad): (u8, bool),
    floor: i16,
    cfg: &LoRaConfig,
) -> bool {
    if receiving {
        return true;
    }
    let thr = i16::from(int_thresh);
    if thr != 0
        && let Ok(rssi) = sx.rssi_inst()
        && rssi > floor + thr
    {
        return true;
    }
    if cad {
        let seen = sx.cad(cfg.sf);
        let _ = sx.start_rx(cfg.preamble);
        return !matches!(seen, Ok(false));
    }
    false
}

pub struct Radio<'d, F: FrontEnd> {
    pub sx: Sx126x<'d>,
    pub fe: F,
    chip: Chip,
    cfg: LoRaConfig,
    watch: RxWatch,
    noise: NoiseFloor,
    next_noise_round: u64,
    transmitting: bool,
    tx_started: Instant,
    /// The SX1262 power in use: the setting within the sub-band's limit
    /// (`Device::radio_tx_dbm`).
    tx_dbm: i8,
    buf: [u8; 255],
}

impl<'d, F: FrontEnd> Radio<'d, F> {
    /// Resets and configures the radio from the prefs, at `tx_dbm`
    /// (`Device::radio_tx_dbm`), then listens. A failure is logged: the node runs on, without
    /// a radio.
    pub fn new(sx: Sx126x<'d>, fe: F, chip: Chip, prefs: &Prefs, tx_dbm: i8) -> Self {
        let mut r = Self {
            sx,
            fe,
            chip,
            cfg: lora_of(prefs),
            watch: RxWatch::new(),
            noise: NoiseFloor::new(),
            next_noise_round: 0,
            transmitting: false,
            tx_started: Instant::now(),
            tx_dbm,
            buf: [0; 255],
        };
        let init =
            r.sx.init(chip.tcxo_millivolts, chip.dcdc)
                .and_then(|()| r.setup());
        if let Err(e) = init {
            error!("SX1262: {e:?}");
        }
        r.fe.apply(prefs);
        r.fe.rx();
        r.watch.configure(&prefs.radio.lora(), prefs.radio.cr);
        r
    }

    fn setup(&mut self) -> Result<(), Error> {
        self.sx.configure(&self.cfg)?;
        self.sx.set_tx_power(self.tx_dbm, self.chip.ocp_ma)?;
        if self.chip.rx_patch {
            self.sx.patch_rx()?;
        }
        self.sx.start_rx(self.cfg.preamble)
    }

    /// The radio settings changed (`Action::RadioChanged`); `tx_dbm` from
    /// `Device::radio_tx_dbm`.
    pub fn reconfigure(&mut self, p: &Prefs, tx_dbm: i8) {
        self.cfg = lora_of(p);
        self.tx_dbm = tx_dbm;
        self.watch.configure(&p.radio.lora(), p.radio.cr);
        self.watch.reset();
        if let Err(e) = self.setup() {
            error!("SX1262: {e:?}");
        }
        self.fe.apply(p);
        if self.transmitting {
            self.fe.rx();
        }
        self.transmitting = false;
    }

    pub fn transmitting(&self) -> bool {
        self.transmitting
    }

    /// Sleeps for hibernation (configuration lost: the boot sets it again).
    pub fn sleep(&mut self) {
        let _ = self.sx.sleep();
        self.fe.sleep();
    }

    /// One pass: what the radio received or finished sending, then the next frame to send,
    /// the noise floor, and the AGC reset when due. Returns what happened to a transmission.
    pub fn poll(
        &mut self,
        dev: &mut Device,
        now: mc_core::Instant,
        rng: &mut impl Rng,
        st: &mut impl Storage,
    ) -> Option<Tx> {
        let mut event = None;
        let flags = self.sx.irq_status().unwrap_or(0);
        let mut cleared = true;
        if flags & irq::TX_DONE != 0 && self.transmitting {
            dev.tx_done(now);
            self.end_tx();
            event = Some(Tx::Done);
        } else if flags & irq::RX_DONE != 0 {
            let crc = flags & irq::CRC_ERR != 0;
            if let Ok(pk) = self.sx.read_packet(&mut self.buf, crc)
                && !crc
            {
                let snr4 = i8::try_from(pk.snr_qdb).unwrap_or(0);
                let n = pk.len.min(self.buf.len());
                dev.on_rx(now, &self.buf[..n], snr4, pk.rssi_dbm, rng, st);
            }
            let _ = self.sx.clear_irq();
        } else if flags & irq::CRC_ERR != 0 {
            let _ = self
                .sx
                .clear_irq_mask(irq::CRC_ERR | rx_watch::HEADER_FLAGS);
        } else {
            cleared = false;
        }
        // A preamble or header the radio left latched: cleared like the C++ does.
        let rx = if cleared || self.transmitting {
            self.watch.reset();
            Watch {
                receiving: false,
                clear: 0,
            }
        } else {
            self.watch.step(flags, Instant::now().as_millis())
        };
        if rx.clear != 0 {
            let _ = self.sx.clear_irq_mask(rx.clear);
        }
        // A transmission that never ended (no TX_DONE): given up after 10 s.
        if self.transmitting && Instant::now() > self.tx_started + Duration::from_secs(10) {
            dev.tx_done(now);
            self.end_tx();
            event = Some(Tx::Done);
        }

        let floor = self.noise.floor();
        let lbt = (
            dev.node.prefs.interference_threshold,
            dev.node.prefs.cad_enabled,
        );
        let (sx, cfg) = (&mut self.sx, &self.cfg);
        if !self.transmitting
            && let Some(f) =
                dev.poll_tx(now, || channel_busy(sx, rx.receiving, lbt, floor, cfg), rng)
        {
            self.fe.tx();
            if self.sx.transmit(self.cfg.preamble, &f.raw).is_ok() {
                self.transmitting = true;
                self.tx_started = Instant::now();
                event = Some(Tx::Started);
            } else {
                self.fe.rx();
            }
        }

        // Noise floor, sampled like the C++ while listening.
        let ms = Instant::now().as_millis();
        if ms >= self.next_noise_round {
            self.noise.trigger();
            self.next_noise_round = ms + mc_core::noise::ROUND_MS;
        }
        let sx = &mut self.sx;
        self.noise
            .step(!self.transmitting, rx.receiving, || sx.rssi_inst().ok());
        dev.noise_floor = self.noise.floor();

        // AGC reset, when set (`agc.reset.interval`): skipped if a packet is coming in or
        // waiting to be read, like the C++.
        if dev.node.agc_due(now) && !self.transmitting && !rx.receiving && flags & irq::RX_DONE == 0
        {
            let r = self.sx.reset_agc().and_then(|()| self.setup());
            if let Err(e) = r {
                error!("AGC reset: {e:?}");
            }
            self.noise.reset();
            self.watch.reset();
        }
        event
    }

    fn end_tx(&mut self) {
        self.transmitting = false;
        self.fe.rx();
        let _ = self.sx.start_rx(self.cfg.preamble);
    }
}
