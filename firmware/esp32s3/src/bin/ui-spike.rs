//! U0 spike (ADR-0008): the `mc-ui` demo screen drawn by Ratatui through mousefood into the
//! e-paper frame, on an `esp-alloc` heap. Logs heap use, render time and refresh latency on
//! UART0 (no companion protocol here).
//!
//! Button A: next item. Button B: switch between the large (25×15) and compact (33×20) grids.
//! Without input, it moves on by itself every 4 s and switches grids every 6 frames.
#![no_std]
#![no_main]

extern crate alloc;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use mc_ui::demo::{Demo, draw};
use mc_ui::panel::{self, COMPACT, Grid, LARGE};
use oxymesh_esp32s3::pca9557::{Pca9557, pin};
use oxymesh_esp32s3::ssd1681::{Frame as EpdFrame, Ssd1681};
use ratatui_core::terminal::Terminal;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// UI heap under test. Bluetooth (MC5) needs about 72 KB more on top of this.
const HEAP_SIZE: usize = 96 * 1024;

static PANEL: StaticCell<panel::Frame> = StaticCell::new();
static EPD: StaticCell<[EpdFrame; 2]> = StaticCell::new();

fn heap() -> (usize, usize) {
    let s = esp_alloc::HEAP.stats();
    (s.current_usage, s.max_usage)
}

#[allow(clippy::large_stack_frames, reason = "spike")]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    println!("ui-spike: heap {} KB", HEAP_SIZE / 1024);

    let out = OutputConfig::default();
    let i2c = I2c::new(
        p.I2C1,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("I2C")
    .with_sda(p.GPIO48)
    .with_scl(p.GPIO47);
    let mut io = Pca9557::new(i2c).ok();
    if let Some(io) = io.as_mut() {
        let _ = io.set(pin::LED_ENABLE, true);
        let _ = io.set(pin::PERIPH_POWER, true);
    }
    Timer::after(Duration::from_millis(10)).await;

    let espi = Spi::new(
        p.SPI3,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(4))
            .with_mode(Mode::_0),
    )
    .expect("display SPI")
    .with_sck(p.GPIO38)
    .with_mosi(p.GPIO45);
    let mut epd = Ssd1681::new(
        espi,
        Output::new(p.GPIO39, Level::High, out),
        Output::new(p.GPIO40, Level::High, out),
        Output::new(p.GPIO41, Level::High, out),
        Input::new(p.GPIO42, InputConfig::default()),
    );
    let _ = epd.init();
    let pull_up = InputConfig::default().with_pull(Pull::Up);
    let btn_a = Input::new(p.GPIO21, pull_up);
    let btn_b = Input::new(p.GPIO14, pull_up);

    let epd_frames = EPD.init([EpdFrame::new(), EpdFrame::new()]);
    let panel_frame = PANEL.init(panel::Frame::new());
    let (h0, _) = heap();
    println!("heap before the terminal: {h0} B");

    let mut demo = Demo {
        selected: 0,
        frame: 0,
        battery_percent: 87,
    };
    let mut grid: Grid = LARGE;
    let mut term = Terminal::new(panel::backend(panel_frame, grid)).expect("terminal");
    let (h1, _) = heap();
    println!(
        "terminal {}x{}: {} B on the heap",
        grid.cols,
        grid.rows,
        h1 - h0
    );

    let (mut a_was, mut b_was) = (false, false);
    let mut next_auto = Instant::now();
    let mut partials = 0u32;
    loop {
        let (a, b) = (btn_a.is_low(), btn_b.is_low());
        let mut go = false;
        let pressed_at = Instant::now();
        if a && !a_was {
            demo.selected += 1;
            go = true;
        }
        // Unattended: switch grids every 6 frames as if B were pressed.
        let auto_switch = demo.frame > 0 && demo.frame % 6 == 0 && Instant::now() >= next_auto;
        if (b && !b_was) || auto_switch {
            grid = if grid == LARGE { COMPACT } else { LARGE };
            drop(term);
            panel_frame.clear();
            let (before, _) = heap();
            term = Terminal::new(panel::backend(panel_frame, grid)).expect("terminal");
            let (after, _) = heap();
            println!(
                "grid {}x{}: terminal now {} B on the heap",
                grid.cols,
                grid.rows,
                after.saturating_sub(h0).max(after - before)
            );
            go = true;
        }
        (a_was, b_was) = (a, b);
        if Instant::now() >= next_auto {
            demo.selected += 1;
            go = true;
        }
        if go && !epd.busy() {
            next_auto = Instant::now() + Duration::from_secs(4);
            demo.frame += 1;
            let t0 = Instant::now();
            let _ = term.draw(|f| draw(f, &demo));
            let t_draw = t0.elapsed().as_millis();
            let [cur, prev] = &mut *epd_frames;
            prev.bits = cur.bits;
            cur.bits = term.backend().display().bits;
            let t1 = Instant::now();
            let full = partials % 30 == 0;
            let _ = if full {
                epd.show_full(cur)
            } else {
                epd.show_partial(cur, prev)
            };
            let t_send = t1.elapsed().as_millis();
            while epd.busy() {
                Timer::after(Duration::from_millis(5)).await;
            }
            let t_refresh = t1.elapsed().as_millis();
            partials += 1;
            let (cur_heap, max_heap) = heap();
            println!(
                "frame {} {}x{} {}: draw {} ms, send {} ms, visible after {} ms (input to visible {} ms); heap {} B, max {} B",
                demo.frame,
                grid.cols,
                grid.rows,
                if full { "full" } else { "partial" },
                t_draw,
                t_send,
                t_refresh,
                pressed_at.elapsed().as_millis(),
                cur_heap,
                max_heap
            );
        }
        Timer::after(Duration::from_millis(5)).await;
    }
}
