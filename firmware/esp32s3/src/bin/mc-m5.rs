//! oxymesh on the ThinkNode M5 — USB companion (MC4).
//!
//! `mc-node` (`Device`) drives the SX1262 and speaks the MeshCore companion protocol on UART0
//! (CH340 bridge, 115,200 baud, `<`/`>` framing). Storage in flash, in the `mcdata` partition.
//! Bring-up screen, buttons, backlight, battery. Logging is off by default: UART0 carries the
//! protocol (`debug-log` feature for debugging).
#![no_std]
#![no_main]
#![deny(clippy::mem_forget)]

use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::iso_8859_1::{FONT_6X13, FONT_10X20};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle};
use embedded_graphics::text::Text;
use esp_backtrace as _;
use esp_bootloader_esp_idf::partitions::{PARTITION_TABLE_MAX_LEN, read_partition_table};
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_storage::FlashStorage;
use log::{error, info};
use mc_air::crypto::LocalIdentity;
use mc_node::device::{Action, Info};
use mc_node::{Device, Event, Prefs};
use mc_proto::{FROM_DEVICE, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame};
use oxymesh_esp32s3::battery::{Smoother, percent};
use oxymesh_esp32s3::flashstore::FlashStore;
use oxymesh_esp32s3::m5;
use oxymesh_esp32s3::pca9557::{Pca9557, pin};
use oxymesh_esp32s3::ssd1681::{Frame, Ssd1681};
use oxymesh_esp32s3::sx126x::{LoRaConfig, Sx126x, irq};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use static_cell::{ConstStaticCell, StaticCell};

esp_bootloader_esp_idf::esp_app_desc!();

const INFO: Info = Info {
    manufacturer: "Elecrow ThinkNode M5",
    version: "v1.18.0-oxymesh",
    build_date: "06-Nov-2026",
    max_tx_power: m5::TX_POWER_MAX,
    has_gps: true,
};

/// The device (≈ 150 KB) lives here from compile time: the stack has no room for it.
static DEVICE: ConstStaticCell<Device> = ConstStaticCell::new(Device::empty(INFO));
static FRAMES: StaticCell<[Frame; 2]> = StaticCell::new();

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

/// Bring-up screen: name, battery, radio. Texts in English.
#[allow(clippy::too_many_arguments)]
fn draw_status(
    f: &mut Frame,
    name: &str,
    mv: u16,
    usb: bool,
    light: bool,
    rx: u32,
    tx: u32,
    contacts: usize,
    queued: usize,
    last: &str,
    freq_khz: u32,
) {
    f.clear();
    let big = MonoTextStyle::new(&FONT_10X20, BinaryColor::On);
    let small = MonoTextStyle::new(&FONT_6X13, BinaryColor::On);
    let mut line = heapless::String::<64>::new();
    let _ = Text::new("oxymesh", Point::new(4, 20), big).draw(f);
    let _ = core::fmt::write(
        &mut line,
        format_args!(
            "{}%{}{}",
            percent(mv),
            if usb { " USB" } else { "" },
            if light { " *" } else { "" }
        ),
    );
    let _ = Text::new(&line, Point::new(122, 20), small).draw(f);
    let _ = Line::new(Point::new(0, 28), Point::new(199, 28))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(f);
    let _ = Text::new(name, Point::new(4, 50), big).draw(f);
    line.clear();
    let _ = core::fmt::write(
        &mut line,
        format_args!("Battery {}.{:02} V", mv / 1000, (mv % 1000) / 10),
    );
    let _ = Text::new(&line, Point::new(4, 76), small).draw(f);
    line.clear();
    let _ = core::fmt::write(&mut line, format_args!("Heard {rx}  Sent {tx}"));
    let _ = Text::new(&line, Point::new(4, 94), small).draw(f);
    line.clear();
    let _ = core::fmt::write(
        &mut line,
        format_args!("Contacts {contacts}  Waiting {queued}"),
    );
    let _ = Text::new(&line, Point::new(4, 112), small).draw(f);
    let _ = Text::new("Last heard:", Point::new(4, 134), small).draw(f);
    let _ = Text::new(last, Point::new(4, 154), big).draw(f);
    line.clear();
    let _ = core::fmt::write(
        &mut line,
        format_args!(
            "{}.{:03} MHz  USB companion",
            freq_khz / 1000,
            freq_khz % 1000
        ),
    );
    let _ = Text::new(&line, Point::new(4, 176), small).draw(f);
    let _ = Text::new(
        if light {
            "Light on (top button: off)"
        } else {
            "Top button: light"
        },
        Point::new(4, 194),
        small,
    )
    .draw(f);
}

#[allow(clippy::large_stack_frames, reason = "packet buffers in main")]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    #[cfg(feature = "debug-log")]
    esp_println::logger::init_logger_from_env();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    info!("oxymesh mc-m5: USB companion");

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
    store.load(&mut dev.node);
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
    let pull_up = InputConfig::default().with_pull(Pull::Up);
    let btn_a = Input::new(p.GPIO21, pull_up);
    let btn_b = Input::new(p.GPIO14, pull_up);
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

    let mut light = false;
    let (mut a_was, mut b_was) = (false, false);
    let mut partials = 0u32;
    let mut next_screen = Instant::now();
    let mut next_battery = Instant::now();
    let mut last_heard: heapless::String<20> = heapless::String::new();
    let _ = last_heard.push_str("-");
    let mut transmitting = false;
    let mut tx_started = Instant::now();
    let mut buf = [0u8; 255];
    let mut rx_bytes = [0u8; 64];
    let mut frame_in = [0u8; mc_proto::FRAME_MAX];
    let mut out_buf = [0u8; USB_MAX];
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
                    next_screen = Instant::now();
                }
                Action::PinChanged => {}
            }
        }

        // User interface.
        while let Some(e) = dev.poll_ui() {
            if let Event::Advert { key, .. } = e
                && let Some(c) = dev
                    .node
                    .contacts
                    .find(&key)
                    .and_then(|s| dev.node.contacts.get(s))
            {
                last_heard.clear();
                let n = mc_node::contacts::name_str(&c.name);
                let _ = last_heard.push_str(
                    core::str::from_utf8(n.get(..n.len().min(18)).unwrap_or(&[])).unwrap_or("?"),
                );
                next_screen = next_screen.min(Instant::now() + Duration::from_secs(2));
            }
        }
        let (a, b) = (btn_a.is_low(), btn_b.is_low());
        if b && !b_was {
            light = !light;
            if let Some(io) = io.as_mut() {
                let _ = io.set(pin::BACKLIGHT, light);
            }
            next_screen = Instant::now();
        }
        if a && !a_was {
            next_screen = Instant::now();
        }
        (a_was, b_was) = (a, b);
        if Instant::now() >= next_battery {
            next_battery = Instant::now() + Duration::from_secs(10);
            let raw_mv = adc.read_blocking(&mut bat_pin);
            dev.battery_mv =
                bat.push((u32::from(raw_mv) * m5::BATTERY_DIVIDER_MILLI / 1000) as u16);
        }
        if Instant::now() >= next_screen && !epd.busy() {
            next_screen = Instant::now() + Duration::from_secs(60);
            let st = dev.node.disp.stats;
            let [cur, prev] = frames;
            *prev = Frame { bits: cur.bits };
            let name_bytes = mc_node::contacts::name_str(&dev.node.prefs.name);
            let name =
                core::str::from_utf8(name_bytes.get(..name_bytes.len().min(18)).unwrap_or(&[]))
                    .unwrap_or("?");
            draw_status(
                cur,
                name,
                dev.battery_mv,
                usb_in.is_high(),
                light,
                st.recv_flood + st.recv_direct,
                st.sent_flood + st.sent_direct,
                dev.node.contacts.list.len(),
                dev.queued_messages(),
                &last_heard,
                dev.node.prefs.radio.freq_khz,
            );
            let _ = if partials % 30 == 0 {
                epd.show_full(cur)
            } else {
                epd.show_partial(cur, prev)
            };
            partials += 1;
        }
        Timer::after(Duration::from_millis(1)).await;
    }
}
