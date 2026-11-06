//! A MeshCore companion connected over USB serial (`docs/spec/companion.md` §1.2), or over
//! Bluetooth LE when the path is `ble:<name>` (§1.1, `crate::ble`).

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use mc_proto::{
    Command, FRAME_MAX, FROM_DEVICE, Response, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame,
};

enum Link {
    Serial {
        port: Box<dyn serialport::SerialPort>,
        deframer: UsbDeframer,
    },
    Ble(crate::ble::Client),
}

pub struct Port {
    pub name: String,
    path: String,
    link: Link,
    /// Frames received but not yet consumed.
    pending: Vec<Vec<u8>>,
    pub verbose: bool,
}

impl Port {
    /// Opens the port. Opening may reset the board (USB-serial bridge): let it boot, then flush
    /// what it wrote.
    pub fn open(path: &str, name: &str) -> Result<Self, String> {
        if let Some(want) = path.strip_prefix("ble:") {
            let c = crate::ble::Client::connect(want, Duration::from_secs(20))?;
            return Ok(Self {
                name: name.into(),
                path: path.into(),
                link: Link::Ble(c),
                pending: Vec::new(),
                verbose: false,
            });
        }
        let port = serialport::new(path, 115_200)
            .timeout(Duration::from_secs(2))
            .open()
            .map_err(|e| format!("{path}: {e}"))?;
        std::thread::sleep(Duration::from_millis(2500));
        let _ = port.clear(serialport::ClearBuffer::Input);
        Ok(Self {
            name: name.into(),
            path: path.into(),
            link: Link::Serial {
                port,
                deframer: UsbDeframer::new(FROM_DEVICE),
            },
            pending: Vec::new(),
            verbose: false,
        })
    }

    /// After the device rebooted: a Bluetooth link is gone and is made again (the bond makes it
    /// quick); the USB bridge stays open through the reset, nothing to do.
    pub fn reconnect(&mut self) -> Result<(), String> {
        if let Link::Ble(_) = self.link
            && let Some(want) = self.path.strip_prefix("ble:")
        {
            self.link = Link::Ble(crate::ble::Client::connect(want, Duration::from_secs(30))?);
            self.pending.clear();
        }
        Ok(())
    }

    pub fn send(&mut self, cmd: &Command<'_>) -> Result<(), String> {
        let mut frame = [0u8; FRAME_MAX];
        let n = cmd.encode(&mut frame).map_err(|_| "command too long")?;
        if self.verbose {
            println!("  {} → {cmd}", self.name);
        }
        match &mut self.link {
            Link::Serial { port, .. } => {
                let mut buf = [0u8; USB_MAX];
                let m =
                    usb_frame(TO_DEVICE, &frame[..n], &mut buf).map_err(|_| "frame too long")?;
                port.write_all(&buf[..m]).map_err(|e| e.to_string())
            }
            Link::Ble(c) => c.write(&frame[..n]),
        }
    }

    /// Reads whatever has arrived, without waiting.
    pub fn poll(&mut self) -> Result<(), String> {
        match &mut self.link {
            Link::Serial { port, deframer } => {
                let n = port.bytes_to_read().map_err(|e| e.to_string())? as usize;
                if n == 0 {
                    return Ok(());
                }
                let mut buf = vec![0u8; n];
                port.read_exact(&mut buf).map_err(|e| e.to_string())?;
                for b in buf {
                    if let Some(f) = deframer.push(b) {
                        self.pending.push(f.to_vec());
                    }
                }
            }
            Link::Ble(c) => self.pending.extend(c.drain()?),
        }
        Ok(())
    }

    /// Takes the frames received so far.
    pub fn drain(&mut self) -> Result<Vec<Vec<u8>>, String> {
        self.poll()?;
        Ok(std::mem::take(&mut self.pending))
    }

    /// Waits up to `secs` seconds for a frame that satisfies `pred`. Other frames stay pending
    /// for a later call.
    pub fn wait<T>(
        &mut self,
        secs: u64,
        mut pred: impl FnMut(&Response<'_>) -> Option<T>,
    ) -> Result<T, String> {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            self.poll()?;
            let mut found = None;
            for (i, f) in self.pending.iter().enumerate() {
                if let Ok(r) = Response::decode(f)
                    && let Some(v) = pred(&r)
                {
                    if self.verbose {
                        println!("  {} ← {r}", self.name);
                    }
                    found = Some((i, v));
                    break;
                }
            }
            if let Some((i, v)) = found {
                self.pending.remove(i);
                return Ok(v);
            }
            if Instant::now() >= end {
                return Err(format!(
                    "{}: expected response not received within {secs} s",
                    self.name
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Sends a command and waits for the first response (code < 0x80).
    pub fn ask(&mut self, cmd: &Command<'_>) -> Result<Vec<u8>, String> {
        self.send(cmd)?;
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            self.poll()?;
            if let Some(i) = self.pending.iter().position(|f| !Response::is_push(f)) {
                let f = self.pending.remove(i);
                if self.verbose {
                    match Response::decode(&f) {
                        Ok(r) => println!("  {} ← {r}", self.name),
                        Err(e) => println!("  {} ← unreadable ({e:?}) {f:02x?}", self.name),
                    }
                }
                return Ok(f);
            }
            if Instant::now() >= end {
                return Err(format!("{}: no response to {cmd}", self.name));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

pub fn now_unix() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}
