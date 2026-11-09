//! Companions over Bluetooth LE from this computer (`docs/spec/companion.md` §1.1): the Nordic
//! UART service, one write per frame on RX, one notification per frame on TX. btleplug runs in
//! a thread of its own; the rest of `mc` stays synchronous.
//!
//! The device asks for security as soon as a central connects. The first time, the system shows
//! its pairing dialog: type the PIN shown on the device's screen. The system keeps the bond.
//!
//! A companion takes one connection and stops advertising while it has one: an app that
//! reconnects at once (a phone app, a web app in a browser) keeps `mc` out. Lookup order: a
//! companion another app of this computer is connected to (CoreBluetooth shares the link), then
//! one `mc` reached before, by the identifier it remembers, then a scan.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _,
    RetrievePeripheralsOptions, ScanFilter, WriteType,
};
use btleplug::platform::{Manager, Peripheral, PeripheralId};
use futures::StreamExt;
use uuid::Uuid;

const NUS: Uuid = Uuid::from_u128(0x6e40_0001_b5a3_f393_e0a9_e50e_24dc_ca9e);
const RX: Uuid = Uuid::from_u128(0x6e40_0002_b5a3_f393_e0a9_e50e_24dc_ca9e);
const TX: Uuid = Uuid::from_u128(0x6e40_0003_b5a3_f393_e0a9_e50e_24dc_ca9e);

/// `MC_BLE_DEBUG=1`: what the lookup tries, on stderr.
fn debug(what: impl FnOnce() -> String) {
    if std::env::var_os("MC_BLE_DEBUG").is_some() {
        eprintln!("ble: {}", what());
    }
}

fn err(e: impl std::fmt::Display) -> String {
    format!("Bluetooth: {e}")
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(err)
}

/// A companion seen advertising the Nordic UART service, or already connected to the system.
#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    pub rssi: Option<i16>,
    /// Held by the system (bonded, reconnected by macOS): it does not advertise.
    pub system: bool,
}

/// Companions the system is connected to, with their names.
async fn held(adapter: &btleplug::platform::Adapter) -> Vec<(Peripheral, String)> {
    let Ok(list) = adapter
        .retrieve_peripherals(RetrievePeripheralsOptions {
            identifiers: None,
            services: Some(vec![NUS]),
        })
        .await
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in list {
        let name = match p.properties().await {
            Ok(Some(props)) => props.local_name.unwrap_or_else(|| "?".into()),
            _ => "?".into(),
        };
        out.push((p, name));
    }
    out
}

/// Companions advertising the Nordic UART service, listened to for `secs` seconds; with `all`,
/// every device heard.
pub fn scan(secs: u64, all: bool) -> Result<Vec<Found>, String> {
    runtime()?.block_on(async {
        let adapter = adapter().await?;
        let mut out: Vec<Found> = held(&adapter)
            .await
            .into_iter()
            .map(|(_, name)| Found {
                name,
                rssi: None,
                system: true,
            })
            .collect();
        let services = if all { Vec::new() } else { vec![NUS] };
        adapter
            .start_scan(ScanFilter { services })
            .await
            .map_err(err)?;
        tokio::time::sleep(Duration::from_secs(secs)).await;
        for p in adapter.peripherals().await.map_err(err)? {
            if let Ok(Some(props)) = p.properties().await
                && (all || props.services.contains(&NUS))
            {
                out.push(Found {
                    name: props.local_name.unwrap_or_else(|| "?".into()),
                    rssi: props.rssi,
                    system: false,
                });
            }
        }
        let _ = adapter.stop_scan().await;
        Ok(out)
    })
}

async fn adapter() -> Result<btleplug::platform::Adapter, String> {
    let manager = Manager::new().await.map_err(err)?;
    manager
        .adapters()
        .await
        .map_err(err)?
        .into_iter()
        .next()
        .ok_or_else(|| err("no adapter"))
}

/// Where `mc` keeps the identifiers the system gave the companions it reached: `uuid name` per
/// line.
fn cache_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(std::path::Path::new(&home).join("Library/Caches/oxymesh/ble-peripherals"))
}

fn remembered() -> Vec<(Uuid, String)> {
    let Some(text) = cache_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| {
            let (id, name) = l.split_once(' ')?;
            Some((Uuid::parse_str(id).ok()?, name.to_string()))
        })
        .collect()
}

fn remember(id: &PeripheralId, name: &str) {
    let Some(path) = cache_path() else { return };
    let Ok(id) = Uuid::parse_str(&id.to_string()) else {
        return;
    };
    let mut all: Vec<(Uuid, String)> = remembered()
        .into_iter()
        .filter(|(i, n)| *i != id && n != name)
        .collect();
    all.insert(0, (id, name.to_string()));
    let text: String = all.iter().map(|(i, n)| format!("{i} {n}\n")).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

/// `want` matches the advertised name, with or without the `MeshCore-` prefix; empty takes
/// the first companion found.
fn matches(name: &str, want: &str) -> bool {
    want.is_empty() || name == want || name.strip_prefix("MeshCore-") == Some(want)
}

/// A companion `mc` reached before, by the identifier CoreBluetooth gave it, whether the system
/// holds it or it is advertising. macOS only: BlueZ names devices by adapter and address, and
/// finds them by scanning.
#[cfg(target_os = "macos")]
async fn retrieve_known(
    adapter: &btleplug::platform::Adapter,
    want: &str,
) -> Option<(Peripheral, Characteristic, String)> {
    for (id, name) in remembered() {
        if !matches(&name, want) {
            continue;
        }
        let list = adapter
            .retrieve_peripherals(RetrievePeripheralsOptions {
                identifiers: Some(vec![PeripheralId::from(id)]),
                services: None,
            })
            .await;
        debug(|| format!("{name} {id}: retrieved {:?}", list.as_ref().map(Vec::len)));
        let Ok(list) = list else { continue };
        for p in list {
            match tokio::time::timeout(Duration::from_secs(8), attach(p.clone(), name.clone()))
                .await
            {
                Ok(Ok(v)) => return Some(v),
                // Out of range or gone: cancel the pending connection, then scan.
                r => {
                    debug(|| format!("{name}: {:?}", r.map(|x| x.map(|_| ()))));
                    let _ = p.disconnect().await;
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
async fn retrieve_known(
    _: &btleplug::platform::Adapter,
    _: &str,
) -> Option<(Peripheral, Characteristic, String)> {
    None
}

/// Finds the companion, connects, subscribes to TX.
async fn open(
    want: &str,
    timeout: Duration,
) -> Result<(Peripheral, Characteristic, String), String> {
    let adapter = adapter().await?;
    if let Some((p, name)) = held(&adapter)
        .await
        .into_iter()
        .find(|(_, n)| matches(n, want))
    {
        return attach(p, name).await;
    }
    if let Some(v) = retrieve_known(&adapter, want).await {
        return Ok(v);
    }
    adapter
        .start_scan(ScanFilter {
            services: vec![NUS],
        })
        .await
        .map_err(err)?;
    let end = Instant::now() + timeout;
    let (p, name) = 'found: loop {
        for p in adapter.peripherals().await.map_err(err)? {
            if let Ok(Some(props)) = p.properties().await
                && props.services.contains(&NUS)
                && let Some(n) = props.local_name
                && matches(&n, want)
            {
                break 'found (p, n);
            }
        }
        if Instant::now() > end {
            let _ = adapter.stop_scan().await;
            return Err(err(format!("no companion named {want:?} in {timeout:?}")));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    let _ = adapter.stop_scan().await;
    attach(p, name).await
}

/// Connects (at once if the system already is), finds RX and TX, subscribes to TX.
async fn attach(
    p: Peripheral,
    name: String,
) -> Result<(Peripheral, Characteristic, String), String> {
    let t0 = Instant::now();
    p.connect().await.map_err(err)?;
    debug(|| format!("{name}: connected after {:?}", t0.elapsed()));
    p.discover_services().await.map_err(err)?;
    debug(|| format!("{name}: services after {:?}", t0.elapsed()));
    let chars = p.characteristics();
    let rx = chars
        .iter()
        .find(|c| c.uuid == RX && c.properties.contains(CharPropFlags::WRITE))
        .cloned()
        .ok_or_else(|| err("no RX characteristic"))?;
    let tx = chars
        .iter()
        .find(|c| c.uuid == TX && c.properties.contains(CharPropFlags::NOTIFY))
        .cloned()
        .ok_or_else(|| err("no TX characteristic"))?;
    // Subscribing writes the CCCD: on a first connection, pairing happens here.
    p.subscribe(&tx).await.map_err(err)?;
    debug(|| format!("{name}: subscribed after {:?}", t0.elapsed()));
    remember(&p.id(), &name);
    Ok((p, rx, name))
}

/// A connected companion. Frames cross two channels to the btleplug thread.
pub struct Client {
    to_dev: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    from_dev: mpsc::Receiver<Vec<u8>>,
}

impl Client {
    /// Connects to the companion named `want` (see [`matches`]); pairing included, which may
    /// wait for a person to type the PIN.
    pub fn connect(want: &str, timeout: Duration) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<String, String>>();
        let (to_dev, mut to_dev_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (from_dev_tx, from_dev) = mpsc::channel::<Vec<u8>>();
        let want = want.to_string();
        let rt = runtime()?;
        std::thread::spawn(move || {
            rt.block_on(async move {
                let (p, rx, name) = match open(&want, timeout).await {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let mut notes = match p.notifications().await {
                    Ok(n) => n,
                    Err(e) => {
                        let _ = ready_tx.send(Err(err(e)));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(name));
                loop {
                    tokio::select! {
                        n = notes.next() => match n {
                            Some(v) if v.uuid == TX => {
                                if from_dev_tx.send(v.value).is_err() {
                                    break;
                                }
                            }
                            Some(_) => {}
                            None => break,
                        },
                        cmd = to_dev_rx.recv() => match cmd {
                            Some(frame) => {
                                if p.write(&rx, &frame, WriteType::WithResponse).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        },
                    }
                }
                let _ = p.disconnect().await;
            });
        });
        // Pairing waits for a person: allow two minutes beyond the scan.
        let name = ready_rx
            .recv_timeout(timeout + Duration::from_secs(120))
            .map_err(|_| err("no answer from the Bluetooth thread"))??;
        eprintln!("Bluetooth: connected to {name}");
        Ok(Self { to_dev, from_dev })
    }

    /// One frame to the device (one write with response).
    pub fn write(&self, frame: &[u8]) -> Result<(), String> {
        self.to_dev
            .send(frame.to_vec())
            .map_err(|_| err("disconnected"))
    }

    /// Frames received so far.
    pub fn drain(&self) -> Result<Vec<Vec<u8>>, String> {
        let mut out = Vec::new();
        loop {
            match self.from_dev.try_recv() {
                Ok(f) => out.push(f),
                Err(mpsc::TryRecvError::Empty) => return Ok(out),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return if out.is_empty() {
                        Err(err("disconnected"))
                    } else {
                        Ok(out)
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_match_with_or_without_the_prefix() {
        assert!(super::matches("MeshCore-Morgan M5", "Morgan M5"));
        assert!(super::matches("MeshCore-Morgan M5", "MeshCore-Morgan M5"));
        assert!(super::matches("MeshCore-Morgan M5", ""));
        assert!(!super::matches("MeshCore-Morgan M5", "Morgan"));
    }
}
