//! Companion over Bluetooth LE (`docs/spec/companion.md` §1.1): Nordic UART service, LE Secure
//! Connections with the PIN shown on screen, bonds kept in flash. The queues and their timing
//! are `mc_proto::ble::BleLink`; this module is the glue to `trouble-host`. Structure taken
//! from loranet (`firmware/esp32s3/src/bin/companion.rs`).
//!
//! Everything runs in the main task (`join` with the main loop): the shared state is plain
//! cells, never borrowed across an await.

use core::cell::{Cell, RefCell};

use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use mc_proto::FRAME_MAX;
use mc_proto::ble::{ADVERT_RESTART_MS, BleLink};
use mc_ui::BleState;
use trouble_host::prelude::*;

/// Longest GAP device name `trouble-host` takes.
pub const GAP_NAME_MAX: usize = 22;
/// Room for a name in the scan response (31 bytes less the AD header).
const SCAN_NAME_MAX: usize = 29;

#[gatt_server]
pub struct Server {
    pub nus: Nus,
}

/// Nordic UART service (BLE:7-9, 45-51): RX written by the app, TX notified to it, both only on
/// an encrypted and authenticated link.
#[gatt_service(uuid = "6e400001-b5a3-f393-e0a9-e50e24dcca9e")]
pub struct Nus {
    /// App → device. Write with response only, like the C++ (`PROPERTY_WRITE`).
    #[characteristic(
        uuid = "6e400002-b5a3-f393-e0a9-e50e24dcca9e",
        write,
        permissions(authenticated)
    )]
    pub rx: heapless::Vec<u8, FRAME_MAX>,
    /// Device → app: read and notify.
    #[characteristic(
        uuid = "6e400003-b5a3-f393-e0a9-e50e24dcca9e",
        read,
        notify,
        permissions(authenticated)
    )]
    pub tx: heapless::Vec<u8, FRAME_MAX>,
}

/// What the main loop asks of the Bluetooth host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Pairing now asks for this PIN.
    Pin(u32),
    /// Forget every paired phone and hang up.
    ForgetAll,
}

/// What the Bluetooth side and the main loop share. The main loop never touches the host
/// itself: without Bluetooth, it runs the same.
pub struct Shared {
    pub link: RefCell<BleLink>,
    pub state: Cell<BleState>,
    /// The PIN pairing asks for.
    pub pin: Cell<u32>,
    /// Paired phones.
    pub paired: Cell<u8>,
    pub control: Channel<NoopRawMutex, Control, 4>,
    /// Frames wait in `link` for a notification.
    pub outbound: Signal<NoopRawMutex, ()>,
    /// New bonds for the main loop to store (it owns the flash).
    pub new_bonds: RefCell<heapless::Deque<BondInformation, 2>>,
    /// End the current connection (paired phones forgotten).
    pub hang_up: Signal<NoopRawMutex, ()>,
}

impl Shared {
    pub const fn new() -> Self {
        Self {
            link: RefCell::new(BleLink::new()),
            state: Cell::new(BleState::Off),
            pin: Cell::new(0),
            paired: Cell::new(0),
            control: Channel::new(),
            outbound: Signal::new(),
            new_bonds: RefCell::new(heapless::Deque::new()),
            hang_up: Signal::new(),
        }
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

/// The 128-bit service UUID, little-endian as it goes on the air.
const NUS_UUID_LE: [u8; 16] = [
    0x9e, 0xca, 0xdc, 0x24, 0x0e, 0xe5, 0xa9, 0xe0, 0x93, 0xf3, 0xa3, 0xb5, 0x01, 0x00, 0x40, 0x6e,
];

/// The GAP device name: the advertised name cut to what `trouble-host` stores, on a character
/// boundary.
pub fn gap_name(full: &str) -> &str {
    let mut end = full.len().min(GAP_NAME_MAX);
    while !full.is_char_boundary(end) {
        end -= 1;
    }
    &full[..end]
}

/// Drives the host. Never returns.
pub async fn run_host<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>)
where
    C::Error: core::fmt::Debug,
{
    loop {
        if let Err(e) = runner.run().await {
            log::warn!("ble: host stopped: {e:?}");
            Timer::after_millis(100).await;
        }
    }
}

/// Carries out the main loop's requests on the host. Never returns.
pub async fn control<C: Controller, P: PacketPool>(stack: &Stack<'_, C, P>, sh: &Shared) {
    loop {
        match sh.control.receive().await {
            Control::Pin(pin) => stack.set_passkey(Some(pin)),
            Control::ForgetAll => {
                let ids: heapless::Vec<Identity, 16> =
                    stack.with_bond_information(|b| b.iter().map(|b| b.identity).collect());
                for id in ids {
                    let _ = stack.remove_bond_information(id);
                }
                sh.paired.set(0);
                sh.hang_up.signal(());
            }
        }
    }
}

/// Advertise, serve one phone until it leaves, wait 1 s, again (BLE:104-110, 218-237).
pub async fn serve<'v, C: Controller>(
    name: &[u8],
    peripheral: &mut Peripheral<'v, C, DefaultPacketPool>,
    server: &Server<'v>,
    sh: &Shared,
) where
    C::Error: core::fmt::Debug,
{
    loop {
        sh.state.set(BleState::Advertising);
        let conn = advertise(name, peripheral, server).await;
        if let Err(e) = &conn {
            log::warn!("ble: advertising failed: {e:?}");
        }
        if let Ok(conn) = conn {
            log::info!("ble: connected");
            // Bonding, and security asked for at once: pairing starts before the first access.
            let _ = conn.raw().set_bondable(true);
            let _ = conn.raw().request_security();
            sh.hang_up.reset();
            match select3(
                events(server, &conn, sh),
                pump(server, &conn, sh),
                sh.hang_up.wait(),
            )
            .await
            {
                Either3::Third(()) => conn.raw().disconnect(),
                Either3::First(()) | Either3::Second(()) => {}
            }
            sh.link.borrow_mut().on_disconnect();
            log::info!("ble: disconnected");
        }
        sh.state.set(BleState::Advertising);
        Timer::after(Duration::from_millis(ADVERT_RESTART_MS)).await;
    }
}

async fn advertise<'v, 's, C: Controller>(
    name: &[u8],
    peripheral: &mut Peripheral<'v, C, DefaultPacketPool>,
    server: &'s Server<'v>,
) -> Result<GattConnection<'v, 's, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut adv = [0; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids128(&[NUS_UUID_LE]),
        ],
        &mut adv[..],
    )?;
    let mut scan = [0; 31];
    let name_ad = if name.len() <= SCAN_NAME_MAX {
        AdStructure::CompleteLocalName(name)
    } else {
        AdStructure::ShortenedLocalName(&name[..SCAN_NAME_MAX])
    };
    let scan_len = AdStructure::encode_slice(&[name_ad], &mut scan[..])?;
    let advertiser = peripheral
        .advertise(
            &AdvertisementParameters::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &adv[..adv_len],
                scan_data: &scan[..scan_len],
            },
        )
        .await?;
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    Ok(conn)
}

fn authenticated(level: SecurityLevel) -> bool {
    matches!(level, SecurityLevel::EncryptedAuthenticated)
}

/// Pairing, encryption and writes on RX, until the phone leaves.
async fn events(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    sh: &Shared,
) {
    let rx = server.nus.rx.handle;
    loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { .. } => return,
            GattConnectionEvent::PassKeyDisplay(_) => {
                log::info!("ble: pairing");
                sh.state.set(BleState::Pairing);
            }
            GattConnectionEvent::PairingComplete {
                security_level,
                bond,
            } => {
                log::info!("ble: paired ({security_level:?}, bond: {})", bond.is_some());
                if authenticated(security_level) {
                    sh.link.borrow_mut().on_authenticated();
                    sh.state.set(BleState::Connected);
                    if let Some(b) = bond {
                        let _ = sh.new_bonds.borrow_mut().push_back(b);
                    }
                    sh.outbound.signal(());
                } else {
                    conn.raw().disconnect();
                }
            }
            GattConnectionEvent::Encrypted { security_level, .. } => {
                log::info!("ble: encrypted ({security_level:?})");
                if authenticated(security_level) {
                    sh.link.borrow_mut().on_authenticated();
                    sh.state.set(BleState::Connected);
                    sh.outbound.signal(());
                }
            }
            // Failed pairing: hang up; advertising comes back after 1 s (BLE:77-88).
            GattConnectionEvent::PairingFailed(e) => {
                log::warn!("ble: pairing failed: {e:?}");
                conn.raw().disconnect();
            }
            GattConnectionEvent::Gatt { event } => {
                if let GattEvent::Write(w) = &event
                    && w.handle() == rx
                {
                    w.with_data(|_, data| {
                        sh.link.borrow_mut().on_write(data);
                    });
                }
                if let Ok(reply) = event.accept() {
                    reply.send().await;
                }
            }
            _ => {}
        }
    }
}

/// The one sender of notifications: a frame per notification, 60 ms apart at least.
async fn pump(server: &Server<'_>, conn: &GattConnection<'_, '_, DefaultPacketPool>, sh: &Shared) {
    let tx = &server.nus.tx;
    let mut buf = [0u8; FRAME_MAX];
    loop {
        let at = sh.link.borrow().notify_at();
        match at {
            None => sh.outbound.wait().await,
            Some(t) => {
                let now = Instant::now().as_millis();
                if t > now {
                    Timer::after(Duration::from_millis(t - now)).await;
                }
                let n = sh
                    .link
                    .borrow_mut()
                    .next_notify(Instant::now().as_millis(), &mut buf);
                if let Some(n) = n {
                    let _ = tx.notify_raw(conn, &buf[..n], true).await;
                }
            }
        }
    }
}
