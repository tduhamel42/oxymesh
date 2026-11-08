//! The ThinkNode M5's GPS (`docs/spec/m5.md` §2), driven like the C++ firmware does it
//! (`MicroNMEALocationProvider` and `EnvironmentSensorManager`, MeshCore, MIT; pins from
//! `variants/thinknode_m5/variant.h`): board v1.0 with an L76K on UART1 at 9600 baud (MCU RX 19,
//! TX 20), EN on GPIO11 (active high), RESET on GPIO13 (active low). On: EN high, RESET pulsed,
//! then `$PCAS04,7` (GPS + BeiDou + GLONASS) after 250 ms. Off: EN low, RESET held.
//!
//! Board v2.0 (AG3352, told apart by the expander's IO7) is wired differently; the C++ does not
//! drive it and neither does this, yet.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::Blocking;
use esp_hal::gpio::Output;
use esp_hal::uart::Uart;
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

pub struct Gps<'d> {
    uart: Uart<'d, Blocking>,
    en: Output<'d>,
    reset: Output<'d>,
    on: bool,
    /// When to send the constellation setting after power-up.
    configure_at: Option<Instant>,
    lines: Lines,
    state: State,
    sync: TimeSync,
}

/// `$` + payload + `*` + checksum + CRLF.
fn sentence(payload: &[u8], out: &mut [u8; 32]) -> usize {
    let sum = payload.iter().fold(0u8, |a, b| a ^ b);
    let hex = b"0123456789ABCDEF";
    let mut n = 0;
    for &b in [b'$'].iter().chain(payload).chain(&[
        b'*',
        hex[usize::from(sum >> 4)],
        hex[usize::from(sum & 15)],
        b'\r',
        b'\n',
    ]) {
        if let Some(o) = out.get_mut(n) {
            *o = b;
            n += 1;
        }
    }
    n
}

impl<'d> Gps<'d> {
    /// Starts off (EN low, RESET held).
    pub fn new(uart: Uart<'d, Blocking>, mut en: Output<'d>, mut reset: Output<'d>) -> Self {
        en.set_low();
        reset.set_low();
        Self {
            uart,
            en,
            reset,
            on: false,
            configure_at: None,
            lines: Lines::new(),
            state: State::default(),
            sync: TimeSync::new(),
        }
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// `begin()` + `reset()` + `configureL76K()`, or `stop()`.
    pub async fn power(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        self.on = on;
        if on {
            self.en.set_high();
            self.reset.set_low();
            Timer::after(Duration::from_millis(10)).await;
            self.reset.set_high();
            self.configure_at = Some(Instant::now() + Duration::from_millis(250));
        } else {
            self.en.set_low();
            self.reset.set_low();
            self.configure_at = None;
            self.state = State::default();
        }
    }

    /// Reads what the receiver sent. The latest valid fix (or `None`), and a time to set the
    /// clock to when a sync is due.
    pub fn poll(&mut self) -> (Option<GpsFix>, Option<u32>) {
        if !self.on {
            return (None, None);
        }
        if self.configure_at.is_some_and(|t| Instant::now() >= t) {
            self.configure_at = None;
            let mut b = [0u8; 32];
            let n = sentence(b"PCAS04,7", &mut b);
            let _ = self.uart.write(&b[..n]);
        }
        let mut buf = [0u8; 64];
        while self.uart.read_ready() {
            let Ok(n) = self.uart.read(&mut buf) else {
                break;
            };
            for &b in &buf[..n] {
                if let Some(s) = self.lines.push(b) {
                    self.state.apply(&s);
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
