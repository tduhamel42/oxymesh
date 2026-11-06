//! Companions over Bluetooth LE from this computer (`docs/spec/companion.md` §1.1): the Nordic
//! UART service, one write per frame on RX, one notification per frame on TX. btleplug runs in
//! a thread of its own; the rest of `mc` stays synchronous.
//!
//! The device asks for security as soon as a central connects. The first time, the system shows
//! its pairing dialog: type the PIN shown on the device's screen. The system keeps the bond.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures::StreamExt;
use uuid::Uuid;

const NUS: Uuid = Uuid::from_u128(0x6e40_0001_b5a3_f393_e0a9_e50e_24dc_ca9e);
const RX: Uuid = Uuid::from_u128(0x6e40_0002_b5a3_f393_e0a9_e50e_24dc_ca9e);
const TX: Uuid = Uuid::from_u128(0x6e40_0003_b5a3_f393_e0a9_e50e_24dc_ca9e);

fn err(e: impl std::fmt::Display) -> String {
    format!("Bluetooth: {e}")
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(err)
}

/// A companion seen advertising the Nordic UART service.
#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    pub rssi: Option<i16>,
}

/// Companions advertising the Nordic UART service, listened to for `secs` seconds; with `all`,
/// every device heard.
pub fn scan(secs: u64, all: bool) -> Result<Vec<Found>, String> {
    runtime()?.block_on(async {
        let adapter = adapter().await?;
        let services = if all { Vec::new() } else { vec![NUS] };
        adapter
            .start_scan(ScanFilter { services })
            .await
            .map_err(err)?;
        tokio::time::sleep(Duration::from_secs(secs)).await;
        let mut out = Vec::new();
        for p in adapter.peripherals().await.map_err(err)? {
            if let Ok(Some(props)) = p.properties().await
                && (all || props.services.contains(&NUS))
            {
                out.push(Found {
                    name: props.local_name.unwrap_or_else(|| "?".into()),
                    rssi: props.rssi,
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

/// `want` matches the advertised name, with or without the `MeshCore-` prefix; empty takes
/// the first companion found.
fn matches(name: &str, want: &str) -> bool {
    want.is_empty() || name == want || name.strip_prefix("MeshCore-") == Some(want)
}

/// Finds the companion, connects, subscribes to TX.
async fn open(
    want: &str,
    timeout: Duration,
) -> Result<(Peripheral, Characteristic, String), String> {
    let adapter = adapter().await?;
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
    p.connect().await.map_err(err)?;
    p.discover_services().await.map_err(err)?;
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
        assert!(super::matches("MeshCore-Tanguy M5", "Tanguy M5"));
        assert!(super::matches("MeshCore-Tanguy M5", "MeshCore-Tanguy M5"));
        assert!(super::matches("MeshCore-Tanguy M5", ""));
        assert!(!super::matches("MeshCore-Tanguy M5", "Tanguy"));
    }
}
