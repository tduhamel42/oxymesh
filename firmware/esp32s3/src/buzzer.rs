//! The M5's passive buzzer on GPIO9 (`docs/spec/m5.md` §2), driven by LEDC: one timer whose
//! frequency follows each note, one channel whose duty sets the volume. Low when idle.
//!
//! Which event plays which tune is `mc_ui::tune`; this task only plays what it is handed.

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use esp_hal::gpio::DriveMode;
use esp_hal::ledc::channel::{self, Channel, ChannelIFace};
use esp_hal::ledc::timer::{self, TimerIFace};
use esp_hal::ledc::{LSGlobalClkSource, Ledc, LowSpeed};
use esp_hal::peripherals::{GPIO9, LEDC};
use esp_hal::time::Rate;
use mc_ui::tune::{Alert, Level, notes};

/// What the main loop hands the buzzer.
pub type Request = Signal<NoopRawMutex, (Alert, Level)>;

/// Duty of each level, %: a passive buzzer is loudest at 50 %.
fn duty(level: Level) -> u8 {
    match level {
        Level::Loud => 50,
        Level::Quiet => 3,
    }
}

/// Plays each request as it comes; a new one cuts the one playing. Never returns.
pub async fn run(ledc: LEDC<'static>, mut pin: GPIO9<'static>, req: &Request) {
    let mut ledc = Ledc::new(ledc);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);
    // The pin goes to the channel at duty 0: low until the first tune.
    let _ = note(&ledc, &mut pin, 1000, 0, 0, req).await;
    let mut next = None;
    loop {
        let (alert, level) = match next.take() {
            Some(r) => r,
            None => req.wait().await,
        };
        for n in notes(alert.tune()) {
            let d = if n.hz > 0 { duty(level) } else { 0 };
            if let Some(r) = note(&ledc, &mut pin, n.hz.max(100), d, n.ms, req).await {
                next = Some(r);
                break;
            }
        }
    }
}

/// One note; a new request cuts it short and is returned.
async fn note(
    ledc: &Ledc<'static>,
    pin: &mut GPIO9<'static>,
    hz: u32,
    duty_pct: u8,
    ms: u32,
    req: &Request,
) -> Option<(Alert, Level)> {
    let mut t = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    let ok = t
        .configure(timer::config::Config {
            duty: timer::config::Duty::Duty10Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: Rate::from_hz(hz),
        })
        .is_ok();
    let mut ch = Channel::<LowSpeed>::new(channel::Number::Channel0, pin.reborrow());
    if ok {
        let _ = ch.configure(channel::config::Config {
            timer: &t,
            duty_pct,
            drive_mode: DriveMode::PushPull,
        });
    }
    let cut = if ms == 0 {
        None
    } else {
        match select(Timer::after_millis(u64::from(ms)), req.wait()).await {
            Either::First(()) => None,
            Either::Second(r) => Some(r),
        }
    };
    let _ = ch.set_duty(0);
    cut
}
