//! oxymesh sur le ThinkNode M5 — mise en route de la radio (MC4).
//!
//! `mc-node` pilote le vrai SX1262 : réception continue, écoute avant émission, émission.
//! Identité de test (pas celle de MeshCore), annonce en zéro saut toutes les minutes, journal sur
//! l'UART0 (pont CH340). Le protocole compagnon viendra ensuite sur le même port.
#![no_std]
#![no_main]
#![deny(clippy::mem_forget)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::iso_8859_1::{FONT_6X13, FONT_10X20};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle};
use embedded_graphics::text::Text;
use esp_backtrace as _;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::Pull;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use log::{error, info, warn};
use mc_air::crypto::LocalIdentity;
use mc_node::{Event, Node, Prefs};
use oxymesh_esp32s3::battery::{Smoother, percent};
use oxymesh_esp32s3::m5;
use oxymesh_esp32s3::pca9557::{Pca9557, pin};
use oxymesh_esp32s3::ssd1681::{Frame, Ssd1681};
use oxymesh_esp32s3::sx126x::{LoRaConfig, Sx126x, irq};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// Région fr (config/regions.toml) : 869,618 MHz, 62,5 kHz, SF8, CR 4/8, préambule 32, mot de
/// synchro privé.
const RADIO: LoRaConfig = LoRaConfig {
    freq_hz: 869_618_000,
    sf: 8,
    bw_hz: 62_500,
    cr: 4,
    preamble: 32,
    sync_word: 0x12,
};
const ADVERT_EVERY: Duration = Duration::from_secs(60);

static NODE: StaticCell<Node> = StaticCell::new();
static FRAMES: StaticCell<[Frame; 2]> = StaticCell::new();

/// Écran de mise en route : nom, batterie, radio. Textes en anglais.
#[allow(clippy::too_many_arguments)]
fn draw_status(
    f: &mut Frame,
    name: &str,
    mv: u16,
    usb: bool,
    light: bool,
    rx: u32,
    tx: u32,
    last: &str,
) {
    f.clear();
    let big = MonoTextStyle::new(&FONT_10X20, BinaryColor::On);
    let small = MonoTextStyle::new(&FONT_6X13, BinaryColor::On);
    let mut line = heapless::String::<64>::new();
    let _ = Text::new("oxymesh", Point::new(4, 20), big).draw(f);
    let _ = core::fmt::write(
        &mut line,
        format_args!("{}%{}", percent(mv), if usb { " USB" } else { "" }),
    );
    let _ = Text::new(&line, Point::new(130, 20), small).draw(f);
    let _ = Line::new(Point::new(0, 28), Point::new(199, 28))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(f);
    let _ = Text::new(name, Point::new(4, 50), big).draw(f);
    line.clear();
    let _ = core::fmt::write(
        &mut line,
        format_args!("Battery {}.{:02} V", mv / 1000, (mv % 1000) / 10),
    );
    let _ = Text::new(&line, Point::new(4, 80), small).draw(f);
    line.clear();
    let _ = core::fmt::write(&mut line, format_args!("Heard {rx}  Sent {tx}"));
    let _ = Text::new(&line, Point::new(4, 100), small).draw(f);
    let _ = Text::new("Last heard:", Point::new(4, 125), small).draw(f);
    let _ = Text::new(last, Point::new(4, 145), big).draw(f);
    let _ = Text::new("EU Narrow 869.618 · fr", Point::new(4, 172), small).draw(f);
    let _ = Text::new(
        if light {
            "Light on"
        } else {
            "Hold top button: light"
        },
        Point::new(4, 192),
        small,
    )
    .draw(f);
}

fn mc_now() -> mc_core::Instant {
    mc_core::Instant::from_nanos(Instant::now().as_micros().saturating_mul(1000))
}

struct Hex<'a>(&'a [u8]);
impl core::fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

#[allow(clippy::large_stack_frames, reason = "tampons de paquet dans main")]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    info!("oxymesh mc-m5 : mise en route");

    let out = OutputConfig::default();
    // Alimentation de la radio (P_LORA_EN).
    let _lora_en = Output::new(p.GPIO46, Level::High, out);
    let i2c = I2c::new(
        p.I2C1,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("I2C")
    .with_sda(p.GPIO48)
    .with_scl(p.GPIO47);
    let mut io = match Pca9557::new(i2c) {
        Ok(io) => Some(io),
        Err(e) => {
            error!("PCA9557 absent ({e:?})");
            None
        }
    };
    if let Some(io) = io.as_mut() {
        let v2 = io.read(pin::GPS_VERSION).unwrap_or(false);
        info!(
            "GPS : révision {}",
            if v2 { "v2.0 (AG3352)" } else { "v1.0 (L76K)" }
        );
        let _ = io.set(pin::LED_ENABLE, true);
    }
    Timer::after(Duration::from_millis(10)).await;

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
    let init = radio
        .init(m5::TCXO_MILLIVOLTS, true)
        .and_then(|()| radio.configure(&RADIO))
        .and_then(|()| radio.set_tx_power(m5::TX_POWER_MAX, m5::OCP_MA))
        .and_then(|()| radio.start_rx(RADIO.preamble));
    match (init, radio.status(), radio.device_errors()) {
        (Ok(()), Ok(s), Ok(e)) => info!("SX1262 prêt : état {s:#04x}, erreurs {e:#06x}"),
        (r, s, e) => error!("SX1262 : {r:?} {s:?} {e:?}"),
    }

    // Écran sur SPI3 : SCK 38, MOSI 45, CS 39, DC 40, RST 41, BUSY 42.
    let espi = Spi::new(
        p.SPI3,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(4))
            .with_mode(Mode::_0),
    )
    .expect("SPI écran")
    .with_sck(p.GPIO38)
    .with_mosi(p.GPIO45);
    let mut epd = Ssd1681::new(
        espi,
        Output::new(p.GPIO39, Level::High, out),
        Output::new(p.GPIO40, Level::High, out),
        Output::new(p.GPIO41, Level::High, out),
        Input::new(p.GPIO42, InputConfig::default()),
    );
    if let Err(e) = epd.init() {
        error!("écran : {e:?}");
    }
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
    let mut light = false;
    let (mut a_was, mut b_was) = (false, false);
    let mut partials = 0u32;
    let mut next_screen = Instant::now();
    let mut last_heard: heapless::String<20> = heapless::String::new();
    let _ = last_heard.push_str("-");

    let hw = esp_hal::rng::Rng::new();
    let mut rng =
        ChaCha20Rng::seed_from_u64((u64::from(hw.random()) << 32) | u64::from(hw.random()));
    let seed: [u8; 32] = mc_air::crypto::sha256_trunc(&[b"oxymesh-m5-dev"]);
    let id = LocalIdentity::from_seed(&seed);
    let mut prefs = Prefs::defaults(id.pub_key());
    prefs.radio.cr = 8;
    prefs.name = [0; 32];
    prefs.name[..10].copy_from_slice(b"oxymesh-m5");
    info!(
        "identité de test {} « oxymesh-m5 »",
        Hex(&id.pub_key()[..4])
    );
    let node = NODE.init(Node::new(id, prefs, 1_793_900_000, mc_now()));

    let mut transmitting = false;
    let mut next_advert = Instant::now() + Duration::from_secs(5);
    let mut buf = [0u8; 255];
    let mut next_status = Instant::now() + Duration::from_secs(30);
    let mut tx_started = Instant::now();
    loop {
        let now = mc_now();
        let flags = radio.irq_status().unwrap_or(0);
        if flags & irq::TX_DONE != 0 && transmitting {
            info!("fin d'émission");
            node.tx_done(now);
            transmitting = false;
            if let Some(io) = io.as_mut() {
                let _ = io.set(pin::LED_NOTIFY, false);
            }
            let _ = radio.start_rx(RADIO.preamble);
        } else if flags & irq::RX_DONE != 0 {
            let crc = flags & irq::CRC_ERR != 0;
            match radio.read_packet(&mut buf, crc) {
                Ok(pk) if !crc => {
                    let snr4 = i8::try_from(pk.snr_qdb).unwrap_or(0);
                    node.on_rx(now, &buf[..pk.len.min(255)], snr4, pk.rssi_dbm, &mut rng);
                }
                Ok(_) => warn!("trame avec CRC faux"),
                Err(e) => error!("lecture : {e:?}"),
            }
            let _ = radio.clear_irq();
        } else if flags & (irq::HEADER_ERR | irq::CRC_ERR) != 0 {
            let _ = radio.clear_irq();
        }
        let busy = !transmitting && flags & irq::HEADER_VALID != 0 && flags & irq::RX_DONE == 0;
        if !transmitting && let Some(f) = node.poll_tx(now, busy, &mut rng) {
            match radio.transmit(RADIO.preamble, &f.raw) {
                Ok(()) => {
                    transmitting = true;
                    tx_started = Instant::now();
                    info!("émission {} o : {}", f.raw.len(), Hex(&f.raw));
                    if let Some(io) = io.as_mut() {
                        let _ = io.set(pin::LED_NOTIFY, true);
                    }
                }
                Err(e) => error!("émission : {e:?}"),
            }
        }
        while let Some(e) = node.poll_event() {
            match e {
                Event::RxLog { snr4, rssi, raw } => {
                    info!("réception snr4={snr4} rssi={rssi} : {}", Hex(&raw))
                }
                Event::Advert { key } => {
                    info!("contact {} (annonce)", Hex(&key[..4]));
                    if let Some(c) = node
                        .contacts
                        .find(&key)
                        .and_then(|sl| node.contacts.get(sl))
                    {
                        last_heard.clear();
                        let n = mc_node::contacts::name_str(&c.name);
                        let _ = last_heard.push_str(
                            core::str::from_utf8(n.get(..n.len().min(18)).unwrap_or(&[]))
                                .unwrap_or("?"),
                        );
                    }
                }
                Event::NewAdvert { contact } => {
                    info!("annonce non ajoutée de {}", Hex(&contact.pub_key[..4]))
                }
                Event::ContactMsg { key, text, .. } => info!(
                    "message de {} : {}",
                    Hex(&key[..4]),
                    core::str::from_utf8(&text).unwrap_or("?")
                ),
                Event::ChannelMsg { idx, text, .. } => info!(
                    "canal {idx} : {}",
                    core::str::from_utf8(&text).unwrap_or("?")
                ),
                Event::SendConfirmed { ack, rtt_ms } => {
                    info!("accusé {} en {rtt_ms} ms", Hex(&ack))
                }
                _ => {}
            }
        }
        if transmitting && Instant::now() > tx_started + Duration::from_secs(5) {
            warn!(
                "émission bloquée : état {:?}, interruptions {flags:#06x} ; retour en réception",
                radio.status()
            );
            node.tx_done(now);
            transmitting = false;
            let _ = radio.start_rx(RADIO.preamble);
        }
        if Instant::now() >= next_status {
            next_status = Instant::now() + Duration::from_secs(30);
            let st = node.disp.stats;
            info!(
                "état : radio {:?}, bruit {:?} dBm, reçues {} (erreurs {}), émises {}",
                radio.status(),
                radio.rssi_inst(),
                st.recv_flood + st.recv_direct,
                st.recv_errors,
                st.sent_flood + st.sent_direct
            );
        }
        // Boutons (actifs bas) : le haut allume ou éteint l'éclairage, le bas redessine l'écran.
        let (a, b) = (btn_a.is_low(), btn_b.is_low());
        if b && !b_was {
            light = !light;
            info!(
                "bouton haut : éclairage {}",
                if light { "allumé" } else { "éteint" }
            );
            if let Some(io) = io.as_mut() {
                let _ = io.set(pin::BACKLIGHT, light);
            }
            next_screen = Instant::now();
        }
        if a && !a_was {
            info!("bouton bas");
            next_screen = Instant::now();
        }
        (a_was, b_was) = (a, b);
        if Instant::now() >= next_screen && !epd.busy() {
            next_screen = Instant::now() + Duration::from_secs(60);
            let raw_mv = adc.read_blocking(&mut bat_pin);
            let mv = bat.push((u32::from(raw_mv) * m5::BATTERY_DIVIDER_MILLI / 1000) as u16);
            let st = node.disp.stats;
            let [cur, prev] = frames;
            *prev = Frame { bits: cur.bits };
            draw_status(
                cur,
                "oxymesh-m5",
                mv,
                usb_in.is_high(),
                light,
                st.recv_flood + st.recv_direct,
                st.sent_flood + st.sent_direct,
                &last_heard,
            );
            let r = if partials % 30 == 0 {
                epd.show_full(cur)
            } else {
                epd.show_partial(cur, prev)
            };
            partials += 1;
            info!(
                "écran ({}) : batterie {} mV ({} %), USB {}, {r:?}",
                if partials == 1 { "complet" } else { "partiel" },
                mv,
                percent(mv),
                usb_in.is_high()
            );
        }
        if Instant::now() >= next_advert {
            next_advert = Instant::now() + ADVERT_EVERY;
            match node.send_self_advert(now, false) {
                Ok(()) => info!("annonce en zéro saut"),
                Err(e) => error!("annonce : {e:?}"),
            }
        }
        Timer::after(Duration::from_millis(1)).await;
    }
}
