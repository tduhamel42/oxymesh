//! oxymesh on the ThinkNode M5: companion over Bluetooth and USB, with its own interface (MC4,
//! MC5, U6).
//!
//! `mc-node` (`Device`) drives the SX1262 and speaks the MeshCore companion protocol over
//! Bluetooth LE (Nordic UART service, PIN pairing, `oxymesh_esp32s3::ble`) and on UART0 (CH340
//! bridge, 115,200 baud, `<`/`>` framing); every frame goes out on both. Storage in flash, in
//! the `mcdata` partition.
//! The screen runs `mc-ui` (Ratatui through mousefood, ADR-0008) on a 64 KB heap: two-button
//! gestures, refresh policy, backlight auto-off, unread LED, hibernation with wake-up on the top
//! button. Logging is off by default: UART0 carries the protocol (`debug-log` feature).
#![no_std]
#![no_main]
#![deny(clippy::mem_forget)]

extern crate alloc;

use core::cell::RefCell;

use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_executor::Spawner;
use embassy_futures::join::{join, join5};
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
use esp_hal::gpio::{
    Event as PinEvent, Input, InputConfig, Level, Output, OutputConfig, Pull, WakeupConfig,
};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_radio::ble::controller::BleConnector;
use esp_storage::FlashStorage;
use log::{error, info};
use mc_air::crypto::LocalIdentity;
use mc_core::noise::NoiseFloor;
use mc_core::rx_watch::{self, RxWatch};
use mc_node::device::{Action, FrontEnd, Info, UiEvent};
use mc_node::{Device, Prefs};
use mc_proto::{FROM_DEVICE, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame};
use mc_spiffs::{Spiffs, cpp};
use mc_ui::input::{Button, Gestures};
use mc_ui::overlay::{self, Overlay, PixelRect};
use mc_ui::panel::{self, COMPACT, Grid, LARGE};
use mc_ui::refresh::{self, Kind, Policy};
use mc_ui::{App, Ble, Ctx, Intent, Outcome, perform};
use oxymesh_esp32s3::battery::{Smoother, percent};
use oxymesh_esp32s3::ble::{self as bt, Control, Server, Shared};
use oxymesh_esp32s3::buzzer;
use oxymesh_esp32s3::flashstore::{BONDS_MAX, FlashStore};
use oxymesh_esp32s3::gps;
use oxymesh_esp32s3::m5;
use oxymesh_esp32s3::partition::{ReadOnly, Region};
use oxymesh_esp32s3::pca9557::{Pca9557, pin};
use oxymesh_esp32s3::pcf8563::Pcf8563;
use oxymesh_esp32s3::ssd1681::{Frame, Ssd1681};
use oxymesh_esp32s3::sx126x::{LoRaConfig, Sx126x, irq};
use oxymesh_esp32s3::tsens::ChipTemp;
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use ratatui_core::terminal::Terminal;
use static_cell::{ConstStaticCell, StaticCell};
use trouble_host::prelude::{
    Address, DefaultPacketPool, ExternalController, GapConfig, HostResources, IoCapabilities,
    PeripheralConfig, appearance,
};

esp_bootloader_esp_idf::esp_app_desc!();

const INFO: Info = Info {
    manufacturer: "Elecrow ThinkNode M5",
    version: "v1.18.0-oxymesh",
    build_date: "06-Nov-2026",
    max_tx_power: m5::TX_POWER_MAX,
    has_gps: true,
    front_end: FrontEnd::None,
};

/// Heap in main DRAM, which it shares with the stack. With the second region: 98 KB for the
/// interface (≈ 43 KB at worst, compose on the compact grid) and Bluetooth (≈ 31 KB idle), and
/// ≈ 68 KB of stack (29 KB used idle). Measured with `debug-log` (`docs/measurements.md`).
const HEAP_SIZE: usize = 24 * 1024;
/// Second heap region, in the RAM the bootloader gives back (`dram2`, 73,744 bytes).
const HEAP_RECLAIMED: usize = 72 * 1024;
/// How often screens get their tick (range test pings, toasts).
const UI_TICK_MS: u64 = 250;
/// The clock at boot until something sets it (2026-11-05); an older clock chip time is not
/// trusted.
const BOOT_EPOCH: u32 = 1_793_900_000;
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
        rx_boosted: p.rx_boosted_gain,
    }
}

/// Listen before talk, when a frame is ready (`RadioLibWrapper::isReceiving`,
/// `RadioLibWrappers.h:44-48` and `.cpp:198-214`): a packet coming in; the channel louder than
/// the noise floor by `int.thresh` (when set); a LoRa preamble found by CAD (when on), after
/// which reception restarts.
fn channel_busy(
    radio: &mut Sx126x<'_>,
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
        && let Ok(rssi) = radio.rssi_inst()
        && rssi > floor + thr
    {
        return true;
    }
    if cad {
        let seen = radio.cad(cfg.sf);
        let _ = radio.start_rx(cfg.preamble);
        return !matches!(seen, Ok(false));
    }
    false
}

fn grid_of(p: &Prefs) -> Grid {
    if p.ui.compact { COMPACT } else { LARGE }
}

/// What the panel receives: the drawn image, inverted and/or turned half a turn.
fn to_panel(src: &Frame, dst: &mut Frame, invert: bool, rotate: bool) {
    for (i, d) in dst.bits.iter_mut().enumerate() {
        let b = if rotate {
            src.bits
                .get(panel::BUF - 1 - i)
                .copied()
                .unwrap_or(0xFF)
                .reverse_bits()
        } else {
            src.bits.get(i).copied().unwrap_or(0xFF)
        };
        *d = if invert { !b } else { b };
    }
}

/// RAM high-water marks, logged with `debug-log`: the main stack is painted at boot and the
/// untouched part measured later; the heap counts its own peak.
#[cfg(feature = "debug-log")]
mod diag {
    unsafe extern "C" {
        static _stack_start_cpu0: u32;
        static _stack_end_cpu0: u32;
    }
    const PAINT: u32 = 0xA5A5_A5A5;
    /// Past the stack guard word (`stack-guard-offset`, 60 bytes).
    const SKIP: usize = 128;

    fn bounds() -> (usize, usize) {
        let start = &raw const _stack_start_cpu0 as usize;
        let end = &raw const _stack_end_cpu0 as usize;
        (end + SKIP, start)
    }

    /// Paints the free stack below the current frame, keeping 1 KB of margin.
    pub fn paint() {
        let (low, _) = bounds();
        let here = 0u32;
        let sp = (&raw const here as usize).saturating_sub(1024) & !3;
        let mut a = low;
        while a < sp {
            // SAFETY: below the live stack frame and above the guard word: nothing lives there.
            unsafe { (a as *mut u32).write_volatile(PAINT) };
            a += 4;
        }
    }

    /// (peak stack use, stack size) in bytes.
    pub fn stack() -> (usize, usize) {
        let (low, high) = bounds();
        let mut a = low;
        // SAFETY: reading the stack region word by word.
        while a < high && unsafe { (a as *const u32).read_volatile() } == PAINT {
            a += 4;
        }
        (high - a, high - low + SKIP)
    }

    pub fn log() {
        let (peak, size) = stack();
        let h = esp_alloc::HEAP.stats();
        let regions: heapless::Vec<(usize, usize), 3> = h
            .region_stats
            .iter()
            .flatten()
            .map(|r| (r.used, r.size))
            .collect();
        log::info!(
            "ram: stack peak {peak} of {size}; heap {} now, {} peak, of {}; regions {:?}",
            h.current_usage,
            h.max_usage,
            h.size,
            regions
        );
    }
}

/// Bluetooth as the screen shows it.
fn ble_of(sh: &Shared) -> Ble {
    Ble {
        state: sh.state.get(),
        pin: sh.pin.get(),
        paired: sh.paired.get(),
    }
}

/// The PIN pairing asks for: the fixed one, or one drawn at boot (`main.cpp:188-202`).
fn pin_of(fixed: u32, hw: &esp_hal::rng::Rng) -> u32 {
    if fixed != 0 {
        fixed
    } else {
        100_000 + hw.random() % 900_000
    }
}

/// What the screen sees for one draw or one input.
fn ui_ctx(dev: &Device, ble: Ble, charging: bool, light: bool) -> Ctx<'_> {
    Ctx {
        dev,
        now_ms: Instant::now().as_millis(),
        unix: dev.node.rtc.now(mc_now()),
        tz_offset_min: dev.node.prefs.ui.tz_offset_min,
        battery_percent: (dev.battery_mv > 0).then(|| percent(dev.battery_mv)),
        charging,
        light,
        gps: dev.gps().map(|f| mc_ui::app::GpsFix {
            lat: f.lat,
            lon: f.lon,
            satellites: f.satellites,
        }),
        ble,
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
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: HEAP_RECLAIMED);
    #[cfg(feature = "debug-log")]
    esp_println::logger::init_logger_from_env();
    #[cfg(feature = "debug-log")]
    diag::paint();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let lpwr = p.LPWR;
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    info!("oxymesh mc-m5: companion over Bluetooth and USB, with its interface");
    // Bluetooth radio first: it also feeds the hardware RNG with entropy.
    // One phone at a time and no scanning: smaller controller pools (4.4 KB of heap). Not
    // `max_connections(1)`: advertising then fails with "Memory Capacity Exceeded".
    let ble_config = esp_radio::ble::Config::default()
        .with_max_connections(2)
        .with_scan_duplicate_list_count(10);
    let connector = BleConnector::new(p.BT, ble_config).ok();

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
    // The clock chip, on the main I2C bus.
    let mut rtc_chip = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .ok()
    .map(|i2c| Pcf8563::new(i2c.with_sda(p.GPIO2).with_scl(p.GPIO1)));
    if let Some(io) = io.as_mut() {
        let _ = io.set(pin::LED_ENABLE, true);
    }
    Timer::after(Duration::from_millis(10)).await;

    // GPS: the board revision from the expander's IO7, read 10 times (high = v2.0, AG3352).
    let highs = (0..10)
        .filter(|_| {
            io.as_mut()
                .and_then(|io| io.read(pin::GPS_VERSION).ok())
                .unwrap_or(false)
        })
        .count();
    let revision = if highs > 5 {
        gps::Revision::V2
    } else {
        gps::Revision::V1
    };
    info!("gps: board {revision:?} ({highs}/10 high)");
    // GPIO10: the GPS switch on board v1.0 (high = on); only read, like the C++.
    #[cfg_attr(not(feature = "debug-log"), allow(unused_variables))]
    let gps_switch = Input::new(p.GPIO10, InputConfig::default());

    let mut gps_rx = match revision {
        gps::Revision::V1 => Uart::new(p.UART1, UartConfig::default().with_baudrate(9600))
            .ok()
            .map(|u| {
                gps::Gps::new(
                    u.with_rx(p.GPIO19).with_tx(p.GPIO20),
                    Output::new(p.GPIO11, Level::Low, OutputConfig::default()),
                    Output::new(p.GPIO13, Level::Low, OutputConfig::default()),
                )
            }),
        gps::Revision::V2 => {
            info!("gps: the AG3352 board is not driven yet");
            None
        }
    };

    // Storage: `mcdata` partition, and the C++ firmware's SPIFFS, read only (ADR-0006).
    let flash = RefCell::new(FlashStorage::new(p.FLASH).multicore_auto_park());
    let mut table = [0u8; PARTITION_TABLE_MAX_LEN];
    let (data_part, cpp_part) = {
        let mut f = flash.borrow_mut();
        let t = read_partition_table(&mut f, &mut table).ok();
        let find = |label: &str| {
            t.as_ref()
                .and_then(|t| t.iter().find(|e| e.label_as_str() == label))
                .map(|e| (e.offset(), e.len()))
        };
        (find("mcdata"), find("spiffs"))
    };
    let Some((data_off, data_len)) = data_part else {
        panic!("no mcdata partition")
    };
    let Some(mut store) = FlashStore::new(
        BlockingAsync::new(Region::new(&flash, data_off, data_len)),
        data_len,
    ) else {
        panic!("mcdata too small")
    };
    let cpp_fs =
        || cpp_part.and_then(|(off, len)| Spiffs::new(ReadOnly::new(&flash, off, len), len).ok());

    let hw = esp_hal::rng::Rng::new();
    let mut rng =
        ChaCha20Rng::seed_from_u64((u64::from(hw.random()) << 32) | u64::from(hw.random()));
    let id = match store.identity() {
        Ok(Some(prv)) => LocalIdentity::from_private(&prv),
        Err(()) => panic!("mcdata unreadable: the identity is left alone"),
        Ok(None) => {
            // First boot: the C++ firmware's node if it left one (ADR-0006), else a new one.
            let imported = cpp_fs().and_then(|mut fs| {
                match cpp::import(&mut fs, mc_node::node::UiPrefs::DEFAULT, &mut store) {
                    Ok(s) => {
                        info!(
                            "import: C++ node, {} contacts, {} channels",
                            s.contacts, s.channels
                        );
                        store.identity().ok().flatten()
                    }
                    Err(e) => {
                        info!("import: nothing from the C++ firmware ({e:?})");
                        None
                    }
                }
            });
            if let Some(prv) = imported {
                LocalIdentity::from_private(&prv)
            } else {
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
        }
    };
    let prefs = Prefs::defaults(id.pub_key());
    let dev = DEVICE.take();
    dev.node.reset(id, prefs, BOOT_EPOCH, mc_now());
    store.load(dev);
    // The clock chip kept the time across the reboot or power-off, unless it lost power.
    match rtc_chip.as_mut().and_then(Pcf8563::read) {
        Some(t) if t >= BOOT_EPOCH => {
            dev.node.rtc.set(mc_now(), t);
            info!("clock: {t} from the PCF8563");
        }
        other => info!("clock: not set ({other:?}), until a phone or the GPS sets it"),
    }
    dev.connected = true; // USB: always connected.

    // Bluetooth: name and PIN fixed at boot, like the C++ (`main.cpp:188-209`).
    let mut mac = [0u8; 6];
    for (d, s) in mac
        .iter_mut()
        .zip(esp_hal::efuse::base_mac_address().as_bytes())
    {
        *d = *s;
    }
    let mut ble_name = [0u8; mc_proto::ble::NAME_MAX];
    let ble_name_len = mc_proto::ble::device_name(
        mc_node::contacts::name_str(&dev.node.prefs.name),
        mac,
        &mut ble_name,
    );
    let ble_name = &ble_name[..ble_name_len];
    let ble_sh = Shared::new();
    ble_sh.pin.set(pin_of(dev.node.prefs.ble_pin, &hw));
    let bonds = store.bonds();
    ble_sh.paired.set(bonds.len() as u8);

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
    let chip_temp = ChipTemp::new();
    Timer::after(Duration::from_millis(1)).await;
    info!("chip temperature: {:?} (0.1 °C)", chip_temp.read());
    let mut buf = [0u8; 255];
    let mut rx_bytes = [0u8; 64];
    let mut frame_in = [0u8; mc_proto::FRAME_MAX];
    let mut out_buf = [0u8; USB_MAX];

    // Interface.
    let panel_frame = PANEL.init(Frame::new());
    let mut app = App::new(grid_of(&dev.node.prefs), true);
    {
        let c = ui_ctx(dev, ble_of(&ble_sh), usb_in.is_high(), light);
        app.start(&c);
    }
    let mut gestures = Gestures::new();
    let mut policy = Policy::new();
    let mut want_draw = true;
    let mut last_minute = u32::MAX;
    let mut last_percent = 0u8;
    let mut next_ui_tick = 0u64;
    let mut next_blink = 0u64;
    let mut blink_off: Option<u64> = None;
    let mut noise = NoiseFloor::new();
    let mut next_noise_round = 0u64;
    let mut watch = RxWatch::new();
    watch.configure(&dev.node.prefs.radio.lora(), dev.node.prefs.radio.cr);
    let mut last_ble = ble_of(&ble_sh);
    // The loop owns the device and its peripherals; the Bluetooth side only shares `ble_sh`,
    // the buzzer only its requests.
    let ble_sh = &ble_sh;
    let buzz = buzzer::Request::new();
    let buzz = &buzz;
    let player = buzzer::run(p.LEDC, p.GPIO9, buzz);
    let main_loop = async move {
        let mut term = Terminal::new(panel::backend(panel_frame, app.grid())).expect("terminal");
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
            loop {
                let n = ble_sh.link.borrow_mut().recv(&mut frame_in);
                let Some(n) = n else { break };
                dev.on_app_frame(now, &frame_in[..n], &mut rng, &mut store);
            }
            while let Some(f) = dev.poll_app() {
                if ble_sh.link.borrow_mut().send(&f) {
                    ble_sh.outbound.signal(());
                }
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
            let mut cleared = true;
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
            } else if flags & irq::CRC_ERR != 0 {
                let _ = radio.clear_irq_mask(irq::CRC_ERR | rx_watch::HEADER_FLAGS);
            } else {
                cleared = false;
            }
            // A preamble or header the radio left latched: cleared like the C++ does.
            let rx = if cleared || transmitting {
                watch.reset();
                rx_watch::Watch {
                    receiving: false,
                    clear: 0,
                }
            } else {
                watch.step(flags, Instant::now().as_millis())
            };
            if rx.clear != 0 {
                let _ = radio.clear_irq_mask(rx.clear);
            }
            if transmitting && Instant::now() > tx_started + Duration::from_secs(10) {
                dev.tx_done(now);
                transmitting = false;
                let _ = radio.start_rx(cfg.preamble);
            }
            let floor = noise.floor();
            let lbt = (
                dev.node.prefs.interference_threshold,
                dev.node.prefs.cad_enabled,
            );
            if !transmitting
                && let Some(f) = dev.poll_tx(
                    now,
                    || channel_busy(&mut radio, rx.receiving, lbt, floor, &cfg),
                    &mut rng,
                )
                && radio.transmit(cfg.preamble, &f.raw).is_ok()
            {
                transmitting = true;
                tx_started = Instant::now();
                if let Some(io) = io.as_mut() {
                    let _ = io.set(pin::LED_NOTIFY, true);
                }
            }
            // GPS: on as the setting says; its fix for the node, its time for the clock.
            if let Some(g) = gps_rx.as_mut() {
                let want = dev.gps_active();
                if want != g.is_on() {
                    g.power(want).await;
                    info!("gps: {}", if want { "on" } else { "off" });
                    want_draw = true;
                }
                let (fix, t) = g.poll();
                if fix.is_some() != dev.gps().is_some() {
                    want_draw = true;
                }
                dev.report_gps(fix);
                if let Some(t) = t {
                    dev.set_time(now, t);
                    info!("gps: clock set to {t}");
                }
            }

            // Noise floor, sampled like the C++ while listening.
            let ms_now = Instant::now().as_millis();
            if ms_now >= next_noise_round {
                noise.trigger();
                next_noise_round = ms_now + mc_core::noise::ROUND_MS;
            }
            noise.step(!transmitting, rx.receiving, || radio.rssi_inst().ok());
            dev.noise_floor = noise.floor();
            dev.tick(now, &mut rng, &mut store);

            // AGC reset, when set (`agc.reset.interval`): skipped if a packet is coming in or
            // waiting to be read, like the C++.
            if dev.node.agc_due(now) && !transmitting && !rx.receiving && flags & irq::RX_DONE == 0 {
                let r = radio
                    .reset_agc()
                    .and_then(|()| radio.configure(&cfg))
                    .and_then(|()| radio.set_tx_power(dev.node.prefs.radio.tx_power_dbm, m5::OCP_MA))
                    .and_then(|()| radio.start_rx(cfg.preamble));
                if let Err(e) = r {
                    error!("AGC reset: {e:?}");
                }
                noise.reset();
                watch.reset();
            }

            // Actions requested by the app.
            let mut power_off = false;
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
                        watch.configure(&dev.node.prefs.radio.lora(), dev.node.prefs.radio.cr);
                        watch.reset();
                        let _ = radio
                            .configure(&cfg)
                            .and_then(|()| {
                                radio.set_tx_power(dev.node.prefs.radio.tx_power_dbm, m5::OCP_MA)
                            })
                            .and_then(|()| radio.start_rx(cfg.preamble));
                        transmitting = false;
                        want_draw = true;
                    }
                    Action::ClockSet => {
                        let t = dev.node.rtc.now(now);
                        if let Some(c) = rtc_chip.as_mut() {
                            let r = c.write(t);
                            info!("clock: {t} to the PCF8563 ({r:?})");
                        }
                        want_draw = true;
                    }
                    Action::PinChanged => {
                        let pin = pin_of(dev.node.prefs.ble_pin, &hw);
                        ble_sh.pin.set(pin);
                        let _ = ble_sh.control.try_send(Control::Pin(pin));
                        want_draw = true;
                    }
                    // `poweroff` from the console: hibernate, as from the menu.
                    Action::PowerOff => power_off = true,
                }
            }

            // A phone just paired: keep its bond.
            loop {
                let b = ble_sh.new_bonds.borrow_mut().pop_front();
                let Some(b) = b else { break };
                store.add_bond(&b);
                ble_sh.paired.set(store.bonds().len() as u8);
            }
            let ble = ble_of(&ble_sh);
            if ble != last_ble {
                last_ble = ble;
                want_draw = true;
            }

            // Battery: smoothed every 10 s.
            if Instant::now() >= next_battery {
                next_battery = Instant::now() + Duration::from_secs(10);
                dev.mcu_temp_deci = chip_temp.read();
                #[cfg(feature = "debug-log")]
                {
                    diag::log();
                    // Boot lines can be lost on the CH340 (FIFO overrun); the state again here.
                    info!(
                        "status: up {} s; gps board {revision:?} ({highs}/10 high) {} (switch {}, {} B, {} sentences, {} errors, {} sats, fix {}); noise {} dBm; chip {:?} (0.1 °C); clock {}",
                        Instant::now().as_secs(),
                        if gps_rx.as_ref().is_some_and(|g| g.is_on()) {
                            "on"
                        } else {
                            "off"
                        },
                        if gps_switch.is_high() { "on" } else { "off" },
                        gps_rx.as_ref().map_or(0, |g| g.bytes),
                        gps_rx.as_ref().map_or(0, |g| g.sentences),
                        gps_rx.as_ref().map_or(0, |g| g.errors),
                        gps_rx.as_ref().map_or(0, |g| g.satellites()),
                        dev.gps().is_some(),
                        dev.noise_floor,
                        dev.mcu_temp_deci,
                        dev.node.rtc.now(mc_now()),
                    );
                }
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
                let c = ui_ctx(dev, ble_of(&ble_sh), charging, light);
                let phone = ble_sh.state.get() == mc_ui::BleState::Connected;
                for e in &events {
                    app.event(e, &c);
                    if let Some(a) = mc_ui::tune::alert(e, dev, phone) {
                        buzz.signal(a);
                    }
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
            let console = power_off.then_some(Intent::Hibernate);
            for i in app.take_intents().into_iter().chain(console) {
                let mut o = perform(&i, dev, mc_now(), &mut rng, &mut store);
                if o == Outcome::CppFind {
                    o = Outcome::Cpp(match cpp_fs() {
                        Some(mut fs) => cpp::summary(&mut fs),
                        None => Err(cpp::ImportError::Spiffs(mc_spiffs::Error::NotSpiffs)),
                    });
                }
                match o {
                    Outcome::CppImport => {
                        if let Some(mut fs) = cpp_fs() {
                            let r = cpp::import(&mut fs, dev.node.prefs.ui, &mut store);
                            info!("import: {r:?}");
                            if r.is_ok() {
                                Timer::after(Duration::from_millis(500)).await;
                                esp_hal::system::software_reset();
                            }
                        }
                    }
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
                    Outcome::ForgetPaired => {
                        store.clear_bonds();
                        ble_sh.paired.set(0);
                        let _ = ble_sh.control.try_send(Control::ForgetAll);
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
                        if let Some(g) = gps_rx.as_mut() {
                            g.power(false).await;
                        }
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
                        let _ = btn_b.apply_wakeup_config(
                            &WakeupConfig::default().with_low_power_path(true),
                        );
                        LowPower::new(lpwr).sleep_deep(RtcSleepConfig::deep());
                    }
                    _ => {}
                }
                let c = ui_ctx(dev, ble_of(&ble_sh), charging, light);
                app.done(&i, &o, &c);
                if app.sync(&c) {
                    drop(term);
                    panel_frame.clear();
                    term =
                        Terminal::new(panel::backend(panel_frame, app.grid())).expect("terminal");
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
            if dev.node.prefs.ui.led_unread && dev.unread_alerting() > 0 && now_ms >= next_blink {
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
            let pct = (dev.battery_mv > 0)
                .then(|| percent(dev.battery_mv))
                .unwrap_or(0);
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
                    let c = ui_ctx(dev, ble_of(&ble_sh), charging, light);
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
    };

    match connector {
        Some(connector) => {
            // Static random address from the MAC (two top bits set): a phone that knew the C++
            // firmware sees a new device and pairs afresh.
            let mut addr = mac;
            addr[5] |= 0xC0;
            let controller: ExternalController<_, 20> = ExternalController::new(connector);
            let mut resources: HostResources<_, DefaultPacketPool, 1, 2, 1, BONDS_MAX> =
                HostResources::new();
            let stack = trouble_host::new(controller, &mut resources)
                .set_random_address(Address::random(addr))
                .set_io_capabilities(IoCapabilities::DisplayOnly)
                .set_passkey(Some(ble_sh.pin.get()))
                .build();
            for b in bonds {
                let _ = stack.add_bond_information(b);
            }
            let mut peripheral = stack.peripheral();
            let runner = stack.runner();
            let gap = core::str::from_utf8(ble_name).map_or("MeshCore", bt::gap_name);
            match Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
                name: gap,
                appearance: &appearance::UNKNOWN,
            })) {
                Ok(server) => {
                    join5(
                        bt::run_host(runner),
                        bt::serve(ble_name, &mut peripheral, &server, &ble_sh),
                        bt::control(&stack, &ble_sh),
                        main_loop,
                        player,
                    )
                    .await;
                }
                Err(_) => {
                    join(main_loop, player).await;
                }
            }
        }
        None => {
            join(main_loop, player).await;
        }
    }
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
