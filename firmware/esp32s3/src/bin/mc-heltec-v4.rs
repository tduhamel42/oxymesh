//! oxymesh on the Heltec WiFi LoRa 32 V4: companion over Bluetooth and USB, with status pages
//! on its OLED (`docs/spec/heltec-v4.md`, `docs/spec/ui.md` §12).
//!
//! `mc-node` (`Device`) drives the SX1262 through its front-end amplifier and speaks the
//! MeshCore companion protocol over Bluetooth LE and on the ESP32-S3's own USB; storage in the
//! `mcdata` partition, the C++ firmware's SPIFFS read once at first boot (ADR-0006). One
//! button: click = next page, double click = previous, long press = act. Logging is off by
//! default: USB carries the protocol (`debug-log` feature).
#![no_std]
#![no_main]
#![deny(clippy::mem_forget)]

extern crate alloc;

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
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
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use esp_radio::ble::controller::BleConnector;
use esp_storage::FlashStorage;
use log::info;
use mc_node::Device;
use mc_node::Prefs;
use mc_node::device::{Action, FrontEnd, Info};
use mc_ui::input::{Button, Gestures};
use mc_ui::status::{Oled, Status};
use mc_ui::{Ble, Ctx, Intent, Outcome, perform};
use oxymesh_esp32s3::battery::{Smoother, percent};
use oxymesh_esp32s3::ble::{Control, Shared};
use oxymesh_esp32s3::heltec_v4::{self as board, Fem, UsbWire};
use oxymesh_esp32s3::run::link::AppLink;
use oxymesh_esp32s3::run::radio::{Chip, Radio, Tx};
use oxymesh_esp32s3::run::{self, boot, mc_now};
use oxymesh_esp32s3::ssd1306::Ssd1306;
use oxymesh_esp32s3::sx126x::Sx126x;
use oxymesh_esp32s3::tsens::ChipTemp;
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use static_cell::{ConstStaticCell, StaticCell};

esp_bootloader_esp_idf::esp_app_desc!();

const INFO: Info = Info {
    // Replaced at boot by the name of the front end found ("Heltec V4.3 OLED").
    manufacturer: "Heltec V4 OLED",
    version: "v1.18.0-oxymesh",
    build_date: "09-Nov-2026",
    max_tx_power: board::TX_POWER_MAX,
    has_gps: false,
    front_end: FrontEnd::Pa,
};

/// Heap in main DRAM for Bluetooth (≈ 31 KB idle) and the status pages (a few KB).
const HEAP_SIZE: usize = 24 * 1024;
/// Second heap region, in the RAM the bootloader gives back.
const HEAP_RECLAIMED: usize = 72 * 1024;
/// The clock at boot until a phone sets it (no clock chip on this board).
const BOOT_EPOCH: u32 = 1_793_900_000;
/// Screen ticks (notes expiring, the clock).
const UI_TICK_MS: u64 = 500;
/// The screen goes dark after this long without a press, unless the settings say otherwise.
const SCREEN_TIMEOUT_MS: u64 = 60_000;
/// The LED blinks this often, this long, while something is unread.
const BLINK_EVERY_MS: u64 = 5_000;
const BLINK_MS: u64 = 80;

static DEVICE: ConstStaticCell<Device> = ConstStaticCell::new(Device::empty(INFO));
static OLED: StaticCell<Oled> = StaticCell::new();

fn ble_of(sh: &Shared) -> Ble {
    Ble {
        state: sh.state.get(),
        pin: sh.pin.get(),
        paired: sh.paired.get(),
    }
}

fn ctx(dev: &Device, ble: Ble, charging: bool) -> Ctx<'_> {
    Ctx {
        dev,
        now_ms: Instant::now().as_millis(),
        unix: dev.node.rtc.now(mc_now()),
        tz_offset_min: dev.node.prefs.ui.tz_offset_min,
        battery_percent: (dev.battery_mv > 0).then(|| percent(dev.battery_mv)),
        charging,
        light: true,
        gps: None,
        ble,
    }
}

#[allow(clippy::large_stack_frames, reason = "packet buffers in main")]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: HEAP_RECLAIMED);
    #[cfg(feature = "debug-log")]
    esp_println::logger::init_logger_from_env();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let lpwr = p.LPWR;
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    info!("oxymesh mc-heltec-v4: companion over Bluetooth and USB, with status pages");
    let ble_config = esp_radio::ble::Config::default()
        .with_max_connections(2)
        .with_scan_duplicate_list_count(10);
    let connector = BleConnector::new(p.BT, ble_config).ok();

    let out = OutputConfig::default();
    // Vext (active low) powers the OLED.
    let _vext = Output::new(p.GPIO36, Level::Low, out);
    Timer::after(Duration::from_millis(5)).await;
    let mut oled_reset = Output::new(p.GPIO21, Level::Low, out);
    Timer::after(Duration::from_millis(2)).await;
    oled_reset.set_high();
    Timer::after(Duration::from_millis(2)).await;
    let mut oled = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .ok()
    .map(|i2c| Ssd1306::new(i2c.with_sda(p.GPIO17).with_scl(p.GPIO18)));
    if let Some(o) = oled.as_mut()
        && o.init().is_err()
    {
        info!("oled: no answer");
        oled = None;
    }

    // Storage: `mcdata`, and the C++ firmware's SPIFFS, read only (ADR-0006).
    let flash = RefCell::new(FlashStorage::new(p.FLASH).multicore_auto_park());
    let (data_part, cpp_part) = boot::partitions(&flash);
    let mut store = boot::open_store(&flash, data_part);
    let cpp_fs = || boot::cpp_fs(&flash, cpp_part);
    let hw = esp_hal::rng::Rng::new();
    let mut rng =
        ChaCha20Rng::seed_from_u64((u64::from(hw.random()) << 32) | u64::from(hw.random()));
    let id = boot::identity(&mut store, cpp_fs, &hw);
    let prefs = Prefs::defaults(id.pub_key());
    let dev = DEVICE.take();
    dev.node.reset(id, prefs, BOOT_EPOCH, mc_now());
    store.load(dev);
    dev.connected = true; // USB: always connected.

    // Front end, then radio.
    let fem = Fem::new(
        p.GPIO7,
        p.GPIO2,
        p.GPIO5,
        p.GPIO46,
        dev.node.prefs.fem_rxgain,
    );
    dev.info.manufacturer = fem.board_name();
    dev.info.front_end = fem.kind;
    info!("front end: {}", fem.board_name());
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(8))
            .with_mode(Mode::_0),
    )
    .expect("SPI")
    .with_sck(p.GPIO9)
    .with_mosi(p.GPIO10)
    .with_miso(p.GPIO11);
    let tx_dbm = dev.radio_tx_dbm();
    let mut radio = Radio::new(
        Sx126x::new(
            spi,
            Output::new(p.GPIO8, Level::High, out),
            Output::new(p.GPIO12, Level::High, out),
            Input::new(p.GPIO13, InputConfig::default()),
            Input::new(p.GPIO14, InputConfig::default()),
        ),
        fem,
        Chip {
            tcxo_millivolts: board::TCXO_MILLIVOLTS,
            dcdc: true,
            ocp_ma: board::OCP_MA,
            rx_patch: true,
        },
        &dev.node.prefs,
        tx_dbm,
    );

    // Bluetooth: name and PIN fixed at boot, like the C++.
    let mac = boot::mac();
    let ble_name = boot::ble_name(&dev.node.prefs, mac);
    let ble_sh = Shared::new();
    ble_sh.pin.set(boot::pin_of(dev.node.prefs.ble_pin, &hw));
    let bonds = store.bonds();
    ble_sh.paired.set(bonds.len() as u8);

    // Button, LED, battery.
    let mut button = Input::new(p.GPIO0, InputConfig::default().with_pull(Pull::Up));
    let mut led = Output::new(p.GPIO35, Level::Low, out);
    let mut adc_ctrl = Output::new(p.GPIO37, Level::Low, out);
    let mut adc_cfg = AdcConfig::new();
    let mut bat_pin = adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<esp_hal::peripherals::ADC1>>(
        p.GPIO1,
        Attenuation::_2p5dB,
    );
    let mut adc = Adc::new(p.ADC1, adc_cfg);
    let mut bat = Smoother::new();
    let chip_temp = ChipTemp::new();

    let mut link = AppLink::new(UsbWire {
        usb: UsbSerialJtag::new(p.USB_DEVICE),
    });

    let frame = OLED.init(Oled::new());
    let mut status = Status::new();
    let mut gestures = Gestures::new();
    let ble_sh = &ble_sh;
    let main_loop = async move {
        let mut shown: Option<mc_ui::status::Screen> = None;
        let mut screen_on = true;
        let mut screen_until = Instant::now().as_millis() + SCREEN_TIMEOUT_MS;
        let mut next_ui_tick = 0u64;
        let mut next_battery = Instant::now();
        let mut next_blink = 0u64;
        let mut blink_off: Option<u64> = None;
        loop {
            let now = mc_now();
            link.poll(dev, ble_sh, now, &mut rng, &mut store);
            match radio.poll(dev, now, &mut rng, &mut store) {
                Some(Tx::Started) => led.set_high(),
                Some(Tx::Done) => led.set_low(),
                None => {}
            }
            dev.tick(now, &mut rng, &mut store);

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
                        let tx = dev.radio_tx_dbm();
                        radio.reconfigure(&dev.node.prefs, tx);
                    }
                    Action::PinChanged => {
                        let pin = boot::pin_of(dev.node.prefs.ble_pin, &hw);
                        ble_sh.pin.set(pin);
                        let _ = ble_sh.control.try_send(Control::Pin(pin));
                    }
                    // No clock chip on this board.
                    Action::ClockSet => {}
                    Action::PowerOff => power_off = true,
                }
            }
            loop {
                let b = ble_sh.new_bonds.borrow_mut().pop_front();
                let Some(b) = b else { break };
                store.add_bond(&b);
                ble_sh.paired.set(store.bonds().len() as u8);
            }

            // Battery and chip temperature every 10 s.
            if Instant::now() >= next_battery {
                next_battery = Instant::now() + Duration::from_secs(10);
                dev.mcu_temp_deci = chip_temp.read();
                adc_ctrl.set_high();
                Timer::after(Duration::from_millis(10)).await;
                let raw_mv = adc.read_blocking(&mut bat_pin);
                adc_ctrl.set_low();
                dev.battery_mv =
                    bat.push((u32::from(raw_mv) * board::BATTERY_MULT_MILLI / 1000) as u16);
            }

            // The button: the first press of a dark screen only lights it.
            let now_ms = Instant::now().as_millis();
            gestures.edge(Button::A, button.is_low(), now_ms);
            let mut intents: heapless::Vec<Intent, 4> = heapless::Vec::new();
            {
                let c = ctx(dev, ble_of(ble_sh), false);
                while let Some(g) = gestures.poll(now_ms) {
                    screen_until = now_ms + SCREEN_TIMEOUT_MS;
                    if !screen_on {
                        screen_on = true;
                        if let Some(o) = oled.as_mut() {
                            let _ = o.power(true);
                        }
                        shown = None;
                        continue;
                    }
                    if let Some(i) = status.gesture(g, &c) {
                        let _ = intents.push(i);
                    }
                }
            }
            if power_off {
                let _ = intents.push(Intent::Hibernate);
            }
            for i in intents {
                let o = perform(&i, dev, mc_now(), &mut rng, &mut store);
                if o == Outcome::Hibernate {
                    dev.flush_contacts(&mut store);
                    if let Some(o) = oled.as_mut() {
                        let _ = o.power(false);
                    }
                    radio.sleep();
                    led.set_low();
                    while button.is_low() {
                        Timer::after(Duration::from_millis(20)).await;
                    }
                    button.listen(PinEvent::LowLevel);
                    let _ = button
                        .apply_wakeup_config(&WakeupConfig::default().with_low_power_path(true));
                    LowPower::new(lpwr).sleep_deep(RtcSleepConfig::deep());
                }
                status.done(&i, &o, now_ms);
            }

            // A phone pairing lights the screen: its PIN is there.
            if ble_sh.state.get() == mc_ui::BleState::Pairing {
                screen_until = now_ms + SCREEN_TIMEOUT_MS;
                if !screen_on {
                    screen_on = true;
                    if let Some(o) = oled.as_mut() {
                        let _ = o.power(true);
                    }
                    shown = None;
                }
            }
            if screen_on && now_ms >= screen_until {
                screen_on = false;
                if let Some(o) = oled.as_mut() {
                    let _ = o.power(false);
                }
            }
            if now_ms >= next_ui_tick {
                next_ui_tick = now_ms + UI_TICK_MS;
                status.tick(now_ms);
            }
            if screen_on && let Some(o) = oled.as_mut() {
                let c = ctx(dev, ble_of(ble_sh), false);
                let s = status.screen(&c);
                if shown.as_ref() != Some(&s) {
                    mc_ui::status::render(&s, frame);
                    let _ = o.show(frame);
                    shown = Some(s);
                }
            }

            // Unread: a short blink every 5 s.
            if dev.node.prefs.ui.led_unread && dev.unread_alerting() > 0 && now_ms >= next_blink {
                next_blink = now_ms + BLINK_EVERY_MS;
                blink_off = Some(now_ms + BLINK_MS);
                led.set_high();
            }
            if blink_off.is_some_and(|t| now_ms >= t) {
                blink_off = None;
                if !radio.transmitting() {
                    led.set_low();
                }
            }
            Timer::after(Duration::from_millis(1)).await;
        }
    };

    run::bt::serve(connector, mac, bonds, &ble_name, ble_sh, main_loop).await;
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
