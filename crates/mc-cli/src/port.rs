//! Un compagnon MeshCore branché en USB série (`docs/spec/companion.md` §1.2).

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use mc_proto::{
    Command, FRAME_MAX, FROM_DEVICE, Response, TO_DEVICE, USB_MAX, UsbDeframer, usb_frame,
};

pub struct Port {
    pub name: String,
    port: Box<dyn serialport::SerialPort>,
    deframer: UsbDeframer,
    /// Trames reçues mais pas encore consommées.
    pending: Vec<Vec<u8>>,
    pub verbose: bool,
}

impl Port {
    /// Ouvre le port. L'ouverture peut redémarrer la carte (pont USB-série) : on la laisse
    /// démarrer, puis on vide ce qu'elle a écrit.
    pub fn open(path: &str, name: &str) -> Result<Self, String> {
        let port = serialport::new(path, 115_200)
            .timeout(Duration::from_secs(2))
            .open()
            .map_err(|e| format!("{path} : {e}"))?;
        std::thread::sleep(Duration::from_millis(2500));
        let _ = port.clear(serialport::ClearBuffer::Input);
        Ok(Self {
            name: name.into(),
            port,
            deframer: UsbDeframer::new(FROM_DEVICE),
            pending: Vec::new(),
            verbose: false,
        })
    }

    pub fn send(&mut self, cmd: &Command<'_>) -> Result<(), String> {
        let mut frame = [0u8; FRAME_MAX];
        let n = cmd.encode(&mut frame).map_err(|_| "commande trop longue")?;
        if self.verbose {
            println!("  {} → {cmd}", self.name);
        }
        let mut buf = [0u8; USB_MAX];
        let m = usb_frame(TO_DEVICE, &frame[..n], &mut buf).map_err(|_| "trame trop longue")?;
        self.port.write_all(&buf[..m]).map_err(|e| e.to_string())
    }

    /// Lit ce qui est arrivé, sans attendre.
    pub fn poll(&mut self) -> Result<(), String> {
        let n = self.port.bytes_to_read().map_err(|e| e.to_string())? as usize;
        if n == 0 {
            return Ok(());
        }
        let mut buf = vec![0u8; n];
        self.port.read_exact(&mut buf).map_err(|e| e.to_string())?;
        for b in buf {
            if let Some(f) = self.deframer.push(b) {
                self.pending.push(f.to_vec());
            }
        }
        Ok(())
    }

    /// Prend les trames reçues jusqu'ici.
    pub fn drain(&mut self) -> Result<Vec<Vec<u8>>, String> {
        self.poll()?;
        Ok(std::mem::take(&mut self.pending))
    }

    /// Attend une trame qui satisfait `pred`, jusqu'à `secs` secondes. Les autres trames restent
    /// en attente pour un appel suivant.
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
                    "{} : pas de réponse attendue en {secs} s",
                    self.name
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Envoie une commande et attend la première réponse (code < 0x80).
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
                        Err(e) => println!("  {} ← illisible ({e:?}) {f:02x?}", self.name),
                    }
                }
                return Ok(f);
            }
            if Instant::now() >= end {
                return Err(format!("{} : pas de réponse à {cmd}", self.name));
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
