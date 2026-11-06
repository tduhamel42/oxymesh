//! oxymesh on the ThinkNode M5: USB companion with its own interface (MC4, U6).
//!
//! `mc-node` (`Device`) drives the SX1262 and speaks the MeshCore companion protocol on UART0
//! (CH340 bridge, 115,200 baud, `<`/`>` framing). Storage in flash, in the `mcdata` partition.
//! The screen runs `mc-ui` (Ratatui through mousefood, ADR-0008) on a 64 KB heap: two-button
//! gestures, refresh policy, backlight auto-off, unread LED, hibernation with wake-up on the top
//! button. Logging is off by default: UART0 carries the protocol (`debug-log` feature).
#![no_std]
#![no_main]
#![deny(clippy::mem_forget)]

extern crate alloc;

use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::iso_8859_1::{FONT_6X13, FONT_10X20};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::Text;
use esp_backtrace as _;
use esp_bootloader_esp_idf::partitions::{PARTITION_TABLE_MAX_LEN, read_partition_table};
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Event as PinEvent, Input, InputConfig, Level, Output, OutputConfig, Pull, WakeupConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_storage::FlashStorage;
use log::{error, info};
use mc_air::crypto::LocalIdentity;
use mc_node::device::{Action, Info, UiEvent};
use mc_node::{Device, Prefs};
use mc_proto::{FROM_DEVICE, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame};
use mc_ui::input::{Button, Gestures};
use mc_ui::overlay::{self, Overlay, PixelRect};
use mc_ui::panel::{self, COMPACT, Grid, LARGE};
use mc_ui::refresh::{self, Kind, Policy};
use mc_ui::{App, Ctx, Outcome, perform};
use oxymesh_esp32s3::battery::{Smoother, percent};
use oxymesh_esp32s3::flashstore::FlashStore;
use oxymesh_esp32s3::m5;
use oxymesh_esp32s3::pca9557::{Pca9557, pin};
use oxymesh_esp32s3::ssd1681::{Frame, Ssd1681};
use oxymesh_esp32s3::sx126x::{LoRaConfig, Sx126x, irq};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use ratatui_core::terminal::Terminal;
use static_cell::{ConstStaticCell, StaticCell};

esp_bootloader_esp_idf::esp_app_desc!();

const INFO: Info = Info {
    manufacturer: "Elecrow ThinkNode M5",
    version: "v1.18.0-oxymesh",
    build_date: "06-Nov-2026",
    max_tx_power: m5::TX_POWER_MAX,
    has_gps: true,
};

/// UI heap (ADR-0008): peak measured at 39 KB on the compact grid.
const HEAP_SIZE: usize = 64 * 1024;
/// How often screens get their tick (range test pings, toasts).
const UI_TICK_MS: u64 = 250;
/// The unread LED blinks this often, this long.
const BLINK_EVERY_MS: u64 = 5_000;
const BLINK_MS: u64 = 80;

/// The device (≈ 150 KB) lives here from compile time: the stack has no room for it.
static DEVICE: ConstStaticCell<Device> = ConstStaticCell::new(Device::empty(INFO));
/// What was sent to the panel: current and previous (the partial refresh needs both).
static FRAMES: StaticCell<[Frame; 2]> = StaticCell::new();
/// What Ratatui draws into.
static PANEL: StaticCell<Frame> = StaticCell::new();

fn mc_now() -> mc_core::Instant {
    mc_core::Instant::from_nanos(Instant::now().as_micros().saturating_mul(1000))
}

/// SX1262 radio settings from the node prefs (preamble 32 if SF ≤ 8, otherwise 16).
fn lora_of(p: &Prefs) -> LoRaConfig {
    LoRaConfig {
        freq_hz: p.radio.freq_khz.saturating_mul(1000),
        sf: p.radio.sf,
        bw_hz: p.radio.bw_hz,
        cr: p.radio.cr.saturating_sub(4).clamp(1, 4),
        preamble: if p.radio.sf <= 8 { 32 } else { 16 },
        sync_word: 0x12,
    }
}

fn grid_of(p: &Prefs) -> Grid {
    if p.ui.compact { COMPACT } else { LARGE }
}

/// What the panel receives: the drawn image, inverted and/or turned half a turn.
fn to_panel(src: &Frame, dst: &mut Frame, invert: bool, rotate: bool) {
    for (i, d) in dst.bits.iter_mut().enumerate() {
        let b = if rotate {
            src.bits.get(panel::BUF - 1 - i).copied().unwrap_or(0xFF).reverse_bits()
        } else {
            src.bits.get(i).copied().unwrap_or(0xFF)
        };
        *d = if invert { !b } else { b };
    }
}

/// What the screen sees for one draw or one input.
fn ui_ctx(dev: &Device, charging: bool, light: bool) -> Ctx<'_> {
    Ctx {
        dev,
        now_ms: Instant::now().as_millis(),
        unix: dev.node.rtc.now(mc_now()),
        tz_offset_min: dev.node.prefs.ui.tz_offset_min,
        battery_percent: (dev.battery_mv > 0).then(|| percent(dev.battery_mv)),
        charging,
        light,
        gps: None,
    }
}

/// The hibernation badge: who this is, our card as a QR, how to wake it.
fn draw_badge(f: &mut Frame, dev: &Device) {
    f.clear();
    let big = MonoTextStyle::new(&FONT_10X20, BinaryColor::On);
    let small = MonoTextStyle::new(&FONT_6X13, BinaryColor::On);
    let name_b = mc_node::contacts::name_str(&dev.node.prefs.name);
    let name = core::str::from_utf8(name_b).unwrap_or("?");
    let _ = Text::new(name, Point::new(4, 20), big).draw(f);
    let link = mc_ui::screens::info_link(name, dev.node.id.pub_key());
    let _ = overlay::draw(
        &Overlay::Qr {
            at: PixelRect {
                x: 0,
                y: 28,
                w: 200,
                h: 146,
            },
            text: link,
        },
        f,
    );
    let _ = Text::new("Off. Press the top button.", Point::new(4, 192), small).draw(f);
}

#[allow(clippy::large_stack_frames, reason = "packet buffers in main")]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    #[cfg(feature = "debug-log")]
    esp_println::logger::init_logger_from_env();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let lpwr = p.LPWR;
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    info!("oxymesh mc-m5: USB companion with its interface");

    let out = OutputConfig::default();
    let _lora_en = Output::new(p.GPIO46, Level::High, out);
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
    }
    Timer::after(Duration::from_millis(10)).await;

    // Storage: `mcdata` partition.
    let mut flash = FlashStorage::new(p.FLASH).multicore_auto_park();
    let mut table = [0u8; PARTITION_TABLE_MAX_LEN];
    let part = read_partition_table(&mut flash, &mut table)
        .ok()
        .and_then(|t| t.iter().find(|e| e.label_as_str() == "mcdata"));
    let Some(part) = part else {
        panic!("no mcdata partition")
    };
    let part_len = part.len();
    let mut region = part.as_flash_region(&mut flash);
    let Ok(nor) = region.as_nor_flash() else {
        panic!("flash")
    };
    let Some(mut store) = FlashStore::new(BlockingAsync::new(nor), part_len) else {
        panic!("mcdata too small")
    };

    let hw = esp_hal::rng::Rng::new();
    let mut rng =
        ChaCha20Rng::seed_from_u64((u64::from(hw.random()) << 32) | u64::from(hw.random()));
    let id = match store.identity() {
        Some(prv) => LocalIdentity::from_private(&prv),
        None => {
            // First boot: new identity (migration from the C++ firmware comes in MC6).
            let id = loop {
                let mut seed = [0u8; 32];
                for c in seed.chunks_exact_mut(4) {
                    c.copy_from_slice(&hw.random().to_le_bytes());
                }
                let id = LocalIdentity::from_seed(&seed);
                if !id.is_reserved() {
                    break id;
                }
            };
            mc_node::Storage::save_identity(&mut store, id.private_key(), id.pub_key());
            id
        }
    };
    let prefs = Prefs::defaults(id.pub_key());
    let dev = DEVICE.take();
    dev.node.reset(id, prefs, 1_793_900_000, mc_now());
    store.load(dev);
    dev.connected = true; // USB: always connected.

    // Radio.
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(8))
            .with_mode(Mode::_0),
    )
    .expect("SPI")
    .with_sck(p.GPIO16)
    .with_mosi(p.GPIO15)
    .with_miso(p.GPIO7);
    let mut radio = Sx126x::new(
        spi,
        Output::new(p.GPIO17, Level::High, out),
        Output::new(p.GPIO6, Level::High, out),
        Input::new(p.GPIO5, InputConfig::default()),
        Input::new(p.GPIO4, InputConfig::default()),
    );
    let mut cfg = lora_of(&dev.node.prefs);
    let init = radio
        .init(m5::TCXO_MILLIVOLTS, true)
        .and_then(|()| radio.configure(&cfg))
        .and_then(|()| radio.set_tx_power(dev.node.prefs.radio.tx_power_dbm, m5::OCP_MA))
        .and_then(|()| radio.start_rx(cfg.preamble));
    if let Err(e) = init {
        error!("SX1262: {e:?}");
    }

    // Display, buttons, battery.
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
    let frames = FRAMES.init([Frame::new(), Frame::new()]);
    if let Some(io) = io.as_mut() {
        let _ = io.set(pin::PERIPH_POWER, true);
    }
    let pull_up = InputConfig::default().with_pull(Pull::Up);
    let btn_a = Input::new(p.GPIO21, pull_up);
    let mut btn_b = Input::new(p.GPIO14, pull_up);
    let usb_in = Input::new(p.GPIO12, InputConfig::default());
    let mut adc_cfg = AdcConfig::new();
    let mut bat_pin = adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<esp_hal::peripherals::ADC1>>(
        p.GPIO8,
        Attenuation::_11dB,
    );
    let mut adc = Adc::new(p.ADC1, adc_cfg);
    let mut bat = Smoother::new();

    // Companion protocol on UART0 (CH340 bridge).
    let mut uart = Uart::new(p.UART0, UartConfig::default())
        .expect("UART0")
        .with_tx(p.GPIO43)
        .with_rx(p.GPIO44);
    let mut deframer = UsbDeframer::new(TO_DEVICE);

    // Heap for the interface only (ADR-0008): the protocol core never allocates.
    let mut light = false;
    // The light went off on its own: the next press only turns it back on.
    let mut dimmed = false;
    let mut light_until = 0u64;
    let mut transmitting = false;
    let mut tx_started = Instant::now();
    let mut next_battery = Instant::now();
    let mut buf = [0u8; 255];
    let mut rx_bytes = [0u8; 64];
    let mut frame_in = [0u8; mc_proto::FRAME_MAX];
    let mut out_buf = [0u8; USB_MAX];

    // Interface.
    let panel_frame = PANEL.init(Frame::new());
    let mut app = App::new(grid_of(&dev.node.prefs), true);
    {
        let c = ui_ctx(dev, usb_in.is_high(), light);
        app.start(&c);
    }
    let mut term = Terminal::new(panel::backend(panel_frame, app.grid())).expect("terminal");
    let mut gestures = Gestures::new();
    let mut policy = Policy::new();
    let mut want_draw = true;
    let mut last_minute = u32::MAX;
    let mut last_percent = 0u8;
    let mut next_ui_tick = 0u64;
    let mut next_blink = 0u64;
    let mut blink_off: Option<u64> = None;
    loop {
        let now = mc_now();

        // Frames from the app.
        while uart.read_ready() {
            let Ok(n) = uart.read(&mut rx_bytes) else {
                break;
            };
            for &b in &rx_bytes[..n] {
                if let Some(f) = deframer.push(b) {
                    let len = f.len();
                    frame_in[..len].copy_from_slice(f);
                    dev.on_app_frame(now, &frame_in[..len], &mut rng, &mut store);
                }
            }
        }
        while let Some(f) = dev.poll_app() {
            if let Ok(n) = usb_frame(FROM_DEVICE, &f, &mut out_buf) {
                let mut sent = 0;
                while sent < n {
                    match uart.write(&out_buf[sent..n]) {
                        Ok(k) => sent += k,
                        Err(_) => break,
                    }
                }
            }
        }

        // Radio.
        let flags = radio.irq_status().unwrap_or(0);
        if flags & irq::TX_DONE != 0 && transmitting {
            dev.tx_done(now);
            transmitting = false;
            if let Some(io) = io.as_mut() {
                let _ = io.set(pin::LED_NOTIFY, false);
            }
            let _ = radio.start_rx(cfg.preamble);
        } else if flags & irq::RX_DONE != 0 {
            let crc = flags & irq::CRC_ERR != 0;
            if let Ok(pk) = radio.read_packet(&mut buf, crc)
                && !crc
            {
                let snr4 = i8::try_from(pk.snr_qdb).unwrap_or(0);
                dev.on_rx(
                    now,
                    &buf[..pk.len.min(255)],
                    snr4,
                    pk.rssi_dbm,
                    &mut rng,
                    &mut store,
                );
            }
            let _ = radio.clear_irq();
        } else if flags & (irq::HEADER_ERR | irq::CRC_ERR) != 0 {
            let _ = radio.clear_irq();
        }
        if transmitting && Instant::now() > tx_started + Duration::from_secs(10) {
            dev.tx_done(now);
            transmitting = false;
            let _ = radio.start_rx(cfg.preamble);
        }
        let busy = !transmitting && flags & irq::HEADER_VALID != 0 && flags & irq::RX_DONE == 0;
        if !transmitting
            && let Some(f) = dev.poll_tx(now, busy, &mut rng)
            && radio.transmit(cfg.preamble, &f.raw).is_ok()
        {
            transmitting = true;
            tx_started = Instant::now();
            if let Some(io) = io.as_mut() {
                let _ = io.set(pin::LED_NOTIFY, true);
            }
        }
        dev.tick(now, &mut rng, &mut store);

        // Actions requested by the app.
        while let Some(a) = dev.poll_action() {
            match a {
                Action::Reboot => {
                    dev.flush_contacts(&mut store);
                    Timer::after(Duration::from_millis(200)).await;
                    esp_hal::system::software_reset();
                }
                Action::FactoryReset => {
                    store.erase_all();
                    Timer::after(Duration::from_millis(1000)).await;
                    esp_hal::system::software_reset();
                }
                Action::RadioChanged => {
                    cfg = lora_of(&dev.node.prefs);
                    let _ = radio
                        .configure(&cfg)
                        .and_then(|()| {
                            radio.set_tx_power(dev.node.prefs.radio.tx_power_dbm, m5::OCP_MA)
                        })
                        .and_then(|()| radio.start_rx(cfg.preamble));
                    transmitting = false;
                    want_draw = true;
                }
                Action::PinChanged => {}
            }
        }

        // Battery: smoothed every 10 s.
        if Instant::now() >= next_battery {
            next_battery = Instant::now() + Duration::from_secs(10);
            let raw_mv = adc.read_blocking(&mut bat_pin);
            dev.battery_mv =
                bat.push((u32::from(raw_mv) * m5::BATTERY_DIVIDER_MILLI / 1000) as u16);
        }

        // Interface: notifications, buttons, timers.
        let now_ms = Instant::now().as_millis();
        let charging = usb_in.is_high();
        gestures.edge(Button::A, btn_a.is_low(), now_ms);
        gestures.edge(Button::B, btn_b.is_low(), now_ms);
        let mut events: heapless::Vec<UiEvent, 32> = heapless::Vec::new();
        while let Some(e) = dev.poll_ui() {
            let _ = events.push(e);
        }
        {
            let c = ui_ctx(dev, charging, light);
            for e in &events {
                app.event(e, &c);
            }
            while let Some(g) = gestures.poll(now_ms) {
                policy.input(now_ms);
                let timeout = u64::from(dev.node.prefs.ui.light_timeout_s) * 1000;
                if dimmed {
                    // The first press after the light went off only turns it back on.
                    dimmed = false;
                    light = true;
                    if let Some(io) = io.as_mut() {
                        let _ = io.set(pin::BACKLIGHT, true);
                    }
                    light_until = now_ms + timeout;
                    want_draw = true;
                    continue;
                }
                if light && timeout > 0 {
                    light_until = now_ms + timeout;
                }
                app.gesture(g, &c);
            }
            if now_ms >= next_ui_tick {
                next_ui_tick = now_ms + UI_TICK_MS;
                app.tick(&c);
            }
        }
        for i in app.take_intents() {
            let o = perform(&i, dev, mc_now(), &mut rng, &mut store);
            match o {
                Outcome::Light => {
                    light = !light;
                    dimmed = false;
                    light_until = now_ms + u64::from(dev.node.prefs.ui.light_timeout_s) * 1000;
                    if let Some(io) = io.as_mut() {
                        let _ = io.set(pin::BACKLIGHT, light);
                    }
                }
                Outcome::Reboot => {
                    Timer::after(Duration::from_millis(200)).await;
                    esp_hal::system::software_reset();
                }
                Outcome::FactoryReset => {
                    store.erase_all();
                    Timer::after(Duration::from_millis(1000)).await;
                    esp_hal::system::software_reset();
                }
                Outcome::Hibernate => {
                    dev.flush_contacts(&mut store);
                    let [cur, _] = &mut *frames;
                    let badge = term.backend_mut().display_mut();
                    draw_badge(badge, dev);
                    let u = dev.node.prefs.ui;
                    to_panel(badge, cur, u.invert, u.rotate);
                    let _ = epd.show_full(cur);
                    while epd.busy() {
                        Timer::after(Duration::from_millis(20)).await;
                    }
                    let _ = epd.sleep();
                    let _ = radio.sleep();
                    if let Some(io) = io.as_mut() {
                        let _ = io.set(pin::BACKLIGHT, false);
                        let _ = io.set(pin::LED_NOTIFY, false);
                        let _ = io.set(pin::PERIPH_POWER, false);
                    }
                    // Wait for the button to be released, then sleep until it is pressed.
                    while btn_b.is_low() {
                        Timer::after(Duration::from_millis(20)).await;
                    }
                    btn_b.listen(PinEvent::LowLevel);
                    let _ = btn_b
                        .apply_wakeup_config(&WakeupConfig::default().with_low_power_path(true));
                    LowPower::new(lpwr).sleep_deep(RtcSleepConfig::deep());
                }
                _ => {}
            }
            let c = ui_ctx(dev, charging, light);
            app.done(&i, &o, &c);
            if app.sync(&c) {
                drop(term);
                panel_frame.clear();
                term = Terminal::new(panel::backend(panel_frame, app.grid())).expect("terminal");
            }
            want_draw = true;
        }

        // Backlight auto-off.
        let timeout = u64::from(dev.node.prefs.ui.light_timeout_s) * 1000;
        if light && timeout > 0 && now_ms >= light_until {
            light = false;
            dimmed = true;
            if let Some(io) = io.as_mut() {
                let _ = io.set(pin::BACKLIGHT, false);
            }
            want_draw = true;
        }

        // Unread: a short blink every 5 s.
        if dev.node.prefs.ui.led_unread && dev.history().unread() > 0 && now_ms >= next_blink {
            next_blink = now_ms + BLINK_EVERY_MS;
            blink_off = Some(now_ms + BLINK_MS);
            if let Some(io) = io.as_mut() {
                let _ = io.set(pin::LED_NOTIFY, true);
            }
        }
        if blink_off.is_some_and(|t| now_ms >= t) {
            blink_off = None;
            if !transmitting && let Some(io) = io.as_mut() {
                let _ = io.set(pin::LED_NOTIFY, false);
            }
        }

        // Drawing: on changes, the minute, a battery step of 5 %; never while BUSY.
        let minute = dev.node.rtc.now(mc_now()) / 60;
        let pct = (dev.battery_mv > 0).then(|| percent(dev.battery_mv)).unwrap_or(0);
        if minute != last_minute || pct.abs_diff(last_percent) >= 5 {
            last_minute = minute;
            last_percent = pct;
            want_draw = true;
        }
        want_draw |= app.take_dirty();
        if !epd.busy() {
            let [cur, prev] = &mut *frames;
            if want_draw {
                want_draw = false;
                let c = ui_ctx(dev, charging, light);
                let _ = term.draw(|f| app.draw(f, &c));
                for o in app.overlays() {
                    let _ = overlay::draw(o, term.backend_mut().display_mut());
                }
                prev.bits = cur.bits;
                let u = dev.node.prefs.ui;
                to_panel(term.backend().display(), cur, u.invert, u.rotate);
                match policy.frame(refresh::hash(&cur.bits), app.take_screen_changed()) {
                    Some(Kind::Full) => {
                        let _ = epd.show_full(cur);
                    }
                    Some(Kind::Partial) => {
                        let _ = epd.show_partial(cur, prev);
                    }
                    None => {}
                }
            } else if policy.idle(now_ms) == Some(Kind::Full) {
                let _ = epd.show_full(cur);
            }
        }
        Timer::after(Duration::from_millis(1)).await;
    }
}
