//! Hardware probe for the M5's GPS (H2): opens UART1 (RX 19, 9600 baud) like `mc-m5`, powers the
//! L76K, counts the bytes it sends, then counts edges on GPIO19's pad directly. Tells a silent
//! receiver from a wiring or setting mistake. Prints on UART0, byte counts only, never positions.
//! On the M5 v1.0 (2026-11-08): about 340 bytes/s.
#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_println::println;
use oxymesh_esp32s3::pca9557::Pca9557;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let mut p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    Timer::after(Duration::from_millis(500)).await;
    println!("gps-probe");

    let i2c = I2c::new(
        p.I2C1,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("I2C")
    .with_sda(p.GPIO48)
    .with_scl(p.GPIO47);
    let mut io = Pca9557::new(i2c).ok();
    let inputs = io.as_mut().and_then(|io| io.inputs().ok());
    println!("expander: {} inputs {inputs:02x?}", io.is_some());
    let switch = Input::new(p.GPIO10.reborrow(), InputConfig::default());
    println!("gpio10 (switch) high: {}", switch.is_high());

    let out = OutputConfig::default();
    let mut en = Output::new(p.GPIO11, Level::Low, out);
    let mut reset = Output::new(p.GPIO13, Level::Low, out);
    // Like mc-m5: the UART first, while the receiver is off.
    let mut u = Uart::new(
        p.UART1.reborrow(),
        UartConfig::default().with_baudrate(9600),
    )
    .expect("uart")
    .with_rx(p.GPIO19.reborrow())
    .with_tx(p.GPIO20.reborrow());
    Timer::after(Duration::from_millis(1000)).await;
    println!(
        "receiver off: ready {} break {} errors {:?}",
        u.read_ready(),
        u.is_break_detected(),
        u.check_for_rx_errors()
    );
    en.set_high();
    reset.set_low();
    Timer::after(Duration::from_millis(10)).await;
    reset.set_high();
    println!("L76K: EN high, RESET released");
    for round in 0..6 {
        let start = Instant::now();
        let (mut n, mut errs) = (0usize, 0usize);
        let mut buf = [0u8; 64];
        while start.elapsed() < Duration::from_millis(2000) {
            while u.read_ready() {
                match u.read(&mut buf) {
                    Ok(k) => n += k,
                    Err(_) => errs += 1,
                }
            }
            Timer::after(Duration::from_millis(5)).await;
        }
        println!(
            "round {round}: {n} bytes, {errs} errors, break {}",
            u.is_break_detected()
        );
        if round == 2 {
            u.clear_break_detected();
            let _ = u.check_for_rx_errors();
            println!("break and errors cleared");
        }
    }
    println!("gpio19 directly:");
    drop(u);
    let pin_in = Input::new(p.GPIO19, InputConfig::default());
    let start = Instant::now();
    let mut last = pin_in.is_high();
    let mut edges = 0u32;
    while start.elapsed() < Duration::from_millis(1500) {
        let v = pin_in.is_high();
        edges += u32::from(v != last);
        last = v;
    }
    println!("gpio19: {edges} edges");
    loop {
        Timer::after(Duration::from_secs(5)).await;
    }
}
