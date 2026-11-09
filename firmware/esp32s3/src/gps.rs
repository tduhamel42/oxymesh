//! The ThinkNode M5's GPS (`docs/spec/m5.md` §2, §6) on UART1 at 9600 baud, switched on and
//! off by the steps of `mc_nmea::receiver`:
//! - board v1.0: an L76K (MCU RX 19, TX 20; EN on GPIO11, RESET on GPIO13), driven like the C++
//!   (`MicroNMEALocationProvider` and `EnvironmentSensorManager`, MeshCore, MIT);
//! - board v2.0 (told apart by the expander's IO7): an AG3352 (MCU RX 20, TX 19) behind a 0.8 V
//!   rail on GPIO10. Neither the C++ nor this has run it on hardware.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::Blocking;
use esp_hal::gpio::{Level, Output};
use esp_hal::uart::Uart;
use mc_nmea::receiver::{Line, Receiver, Step, sentence};
use mc_nmea::{Gps as State, Lines, TimeSync};
use mc_node::device::GpsFix;

/// Board revision, from PCA9557 IO7 (high = v2.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revision {
    /// L76K.
    V1,
    /// AG3352.
    V2,
}

/// The receiver's control lines, as the board wires them.
#[derive(Default)]
pub struct Pins<'d> {
    pub enable: Option<Output<'d>>,
    pub reset: Option<Output<'d>>,
    pub rail: Option<Output<'d>>,
}

impl<'d> Pins<'d> {
    fn get(&mut self, l: Line) -> Option<&mut Output<'d>> {
        match l {
            Line::Enable => self.enable.as_mut(),
            Line::Reset => self.reset.as_mut(),
            Line::Rail => self.rail.as_mut(),
        }
    }
}

impl Revision {
    pub fn receiver(self) -> Receiver {
        match self {
            Revision::V1 => Receiver::L76k,
            Revision::V2 => Receiver::Ag3352,
        }
    }
}

pub struct Gps<'d> {
    uart: Uart<'d, Blocking>,
    receiver: Receiver,
    pins: Pins<'d>,
    on: bool,
    /// A sentence to send once the receiver is up.
    configure_at: Option<(Instant, &'static [u8])>,
    lines: Lines,
    state: State,
    sync: TimeSync,
    /// Bytes, RMC/GGA sentences and read errors since boot (diagnostics).
    pub bytes: u32,
    pub sentences: u32,
    pub errors: u32,
}

impl<'d> Gps<'d> {
    /// Starts off.
    pub fn new(uart: Uart<'d, Blocking>, receiver: Receiver, pins: Pins<'d>) -> Self {
        let mut g = Self {
            uart,
            receiver,
            pins,
            on: false,
            configure_at: None,
            lines: Lines::new(),
            state: State::default(),
            sync: TimeSync::new(),
            bytes: 0,
            sentences: 0,
            errors: 0,
        };
        g.apply_off();
        g
    }

    fn apply_off(&mut self) {
        for s in self.receiver.off() {
            if let Step::Set(l, v) = *s
                && let Some(p) = self.pins.get(l)
            {
                p.set_level(if v { Level::High } else { Level::Low });
            }
        }
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Switches the receiver on or off (`mc_nmea::receiver`).
    pub async fn power(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        self.on = on;
        self.configure_at = None;
        let steps = if on {
            self.receiver.on()
        } else {
            self.receiver.off()
        };
        for s in steps {
            match *s {
                Step::Set(l, v) => {
                    if let Some(p) = self.pins.get(l) {
                        p.set_level(if v { Level::High } else { Level::Low });
                    }
                }
                Step::Wait(ms) => Timer::after(Duration::from_millis(ms)).await,
                Step::Later(ms, payload) => {
                    self.configure_at = Some((Instant::now() + Duration::from_millis(ms), payload));
                }
            }
        }
        if !on {
            self.state = State::default();
        }
    }

    /// Reads what the receiver sent. The latest valid fix (or `None`), and a time to set the
    /// clock to when a sync is due.
    pub fn poll(&mut self) -> (Option<GpsFix>, Option<u32>) {
        if !self.on {
            return (None, None);
        }
        if let Some((t, payload)) = self.configure_at
            && Instant::now() >= t
        {
            self.configure_at = None;
            let mut b = [0u8; 32];
            let n = sentence(payload, &mut b);
            let _ = self.uart.write(&b[..n]);
        }
        let mut buf = [0u8; 64];
        for _ in 0..8 {
            if !self.uart.read_ready() {
                break;
            }
            // An error (glitch, overflow) is reported once and cleared; the bytes stay.
            let n = match self.uart.read(&mut buf) {
                Ok(n) => n,
                Err(_) => {
                    self.errors = self.errors.wrapping_add(1);
                    continue;
                }
            };
            self.bytes = self.bytes.wrapping_add(n as u32);
            for &b in &buf[..n] {
                if let Some(s) = self.lines.push(b) {
                    self.state.apply(&s);
                    self.sentences = self.sentences.wrapping_add(1);
                }
            }
        }
        let fix = self.state.pos.filter(|_| self.state.valid).map(|p| GpsFix {
            lat: p.lat,
            lon: p.lon,
            alt_mm: self.state.alt_mm,
            satellites: self.state.satellites,
        });
        let time = self.sync.poll(Instant::now().as_millis(), &self.state);
        (fix, time)
    }

    /// Satellites in view of the last GGA, fix or not.
    pub fn satellites(&self) -> u8 {
        self.state.satellites
    }
}
