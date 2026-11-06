//! Companion framing over USB serial (`docs/spec/companion.md` §1.2): `'<'` or `'>'`,
//! little-endian `u16` length, frame. Over Bluetooth, each write or notification is already a
//! frame.

use crate::codec::FRAME_MAX;
use mc_core::{Overflow, Writer};

/// Start of a frame from the app to the device.
pub const TO_DEVICE: u8 = b'<';
/// Start of a frame from the device to the app.
pub const FROM_DEVICE: u8 = b'>';
/// Maximum size of a framed frame.
pub const USB_MAX: usize = FRAME_MAX + 3;

/// Writes `start ‖ len ‖ frame` into `out`. Returns the length written.
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

/// Recovers frames from a byte stream, like `ArduinoSerialInterface::checkRecvFrame`
/// (ASI:38-73): anything before the start byte is ignored (boot log, noise), as is a zero length;
/// a frame announced as longer than 176 bytes is read in full and truncated to 176.
#[derive(Debug, Clone)]
pub struct UsbDeframer {
    start: u8,
    state: State,
    len: usize,
    at: usize,
    buf: [u8; FRAME_MAX],
}

impl UsbDeframer {
    /// `start`: [`FROM_DEVICE`] on the app side, [`TO_DEVICE`] on the device side.
    pub const fn new(start: u8) -> Self {
        Self {
            start,
            state: State::Idle,
            len: 0,
            at: 0,
            buf: [0; FRAME_MAX],
        }
    }

    /// Feeds one byte. Returns a frame once it is complete.
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
