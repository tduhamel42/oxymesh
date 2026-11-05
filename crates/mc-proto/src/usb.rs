//! Tramage du compagnon sur USB série (`docs/spec/companion.md` §1.2) : `'<'` ou `'>'`, longueur
//! `u16` petit-boutiste, trame. En Bluetooth, chaque écriture ou notification est déjà une trame.

use crate::codec::FRAME_MAX;
use mc_core::{Overflow, Writer};

/// Début d'une trame de l'app vers l'appareil.
pub const TO_DEVICE: u8 = b'<';
/// Début d'une trame de l'appareil vers l'app.
pub const FROM_DEVICE: u8 = b'>';
/// Taille maximale d'une trame tramée.
pub const USB_MAX: usize = FRAME_MAX + 3;

/// Écrit `start ‖ len ‖ frame` dans `out`. Renvoie la longueur écrite.
pub fn usb_frame(start: u8, frame: &[u8], out: &mut [u8]) -> Result<usize, Overflow> {
    if frame.is_empty() || frame.len() > FRAME_MAX {
        return Err(Overflow);
    }
    let mut w = Writer::new(out);
    w.put_u8(start)?;
    w.put_u16_le(u16::try_from(frame.len()).map_err(|_| Overflow)?)?;
    w.put(frame)?;
    Ok(w.position())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Len1,
    Len2(u8),
    Body,
}

/// Retrouve les trames dans un flux d'octets, comme `ArduinoSerialInterface::checkRecvFrame`
/// (ASI:38-73) : tout ce qui précède l'octet de début est ignoré (journal de démarrage, bruit),
/// comme une longueur nulle ; une trame annoncée plus longue que 176 octets est lue en entier et
/// tronquée à 176.
#[derive(Debug, Clone)]
pub struct UsbDeframer {
    start: u8,
    state: State,
    len: usize,
    at: usize,
    buf: [u8; FRAME_MAX],
}

impl UsbDeframer {
    /// `start` : [`FROM_DEVICE`] côté app, [`TO_DEVICE`] côté appareil.
    pub const fn new(start: u8) -> Self {
        Self {
            start,
            state: State::Idle,
            len: 0,
            at: 0,
            buf: [0; FRAME_MAX],
        }
    }

    /// Ajoute un octet. Renvoie une trame quand elle est complète.
    pub fn push(&mut self, b: u8) -> Option<&[u8]> {
        match self.state {
            State::Idle => {
                if b == self.start {
                    self.state = State::Len1;
                }
                None
            }
            State::Len1 => {
                self.state = State::Len2(b);
                None
            }
            State::Len2(lo) => {
                let len = usize::from(u16::from_le_bytes([lo, b]));
                if len == 0 {
                    self.state = State::Idle;
                } else {
                    self.len = len;
                    self.at = 0;
                    self.state = State::Body;
                }
                None
            }
            State::Body => {
                if let Some(slot) = self.buf.get_mut(self.at) {
                    *slot = b;
                }
                self.at += 1;
                if self.at < self.len {
                    return None;
                }
                self.state = State::Idle;
                self.buf.get(..self.len.min(FRAME_MAX))
            }
        }
    }
}
