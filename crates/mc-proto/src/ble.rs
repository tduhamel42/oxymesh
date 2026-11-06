//! Companion link over Bluetooth (`docs/spec/companion.md` §1.1), as `SerialBLEInterface`
//! (`src/helpers/esp32/SerialBLEInterface.cpp`, BLE:114-237): one write or notification is one
//! frame, four frames are queued each way, at most one notification goes out every 60 ms, and
//! only an authenticated link carries frames.
//!
//! Pure bookkeeping: the firmware's Bluetooth host feeds writes in and sends what comes out.

use crate::codec::FRAME_MAX;

/// Frames queued in each direction (`FRAME_QUEUE_SIZE`).
pub const QUEUE: usize = 4;
/// Minimum time between two notifications (`BLE_WRITE_MIN_INTERVAL`, BLE:181).
pub const NOTIFY_GAP_MS: u64 = 60;
/// Advertising restarts this long after a disconnection or a failed pairing (BLE:11).
pub const ADVERT_RESTART_MS: u64 = 1000;
/// Prefix of the advertised name (`BLE_NAME_PREFIX`).
pub const NAME_PREFIX: &[u8] = b"MeshCore-";
/// Longest advertised name: prefix and a 31-byte node name.
pub const NAME_MAX: usize = NAME_PREFIX.len() + 31;
/// Nordic UART service (BLE:7).
pub const SERVICE_UUID: &str = "6E400001-B5A3-F393-E0A9-E50E24DCCA9E";
/// App → device characteristic (BLE:8).
pub const RX_UUID: &str = "6E400002-B5A3-F393-E0A9-E50E24DCCA9E";
/// Device → app characteristic (BLE:9).
pub const TX_UUID: &str = "6E400003-B5A3-F393-E0A9-E50E24DCCA9E";

#[derive(Debug, Clone, Copy)]
struct Slot {
    len: usize,
    buf: [u8; FRAME_MAX],
}

/// A FIFO of [`QUEUE`] frames.
#[derive(Debug, Clone)]
struct Ring {
    slots: [Slot; QUEUE],
    head: usize,
    len: usize,
}

impl Ring {
    const fn new() -> Self {
        Self {
            slots: [Slot {
                len: 0,
                buf: [0; FRAME_MAX],
            }; QUEUE],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, frame: &[u8]) -> bool {
        if self.len >= QUEUE || frame.len() > FRAME_MAX {
            return false;
        }
        let Some(slot) = self.slots.get_mut((self.head + self.len) % QUEUE) else {
            return false;
        };
        let Some(dst) = slot.buf.get_mut(..frame.len()) else {
            return false;
        };
        dst.copy_from_slice(frame);
        slot.len = frame.len();
        self.len += 1;
        true
    }

    fn pop(&mut self, out: &mut [u8; FRAME_MAX]) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        let slot = self.slots.get(self.head)?;
        let n = slot.len;
        out.get_mut(..n)?.copy_from_slice(slot.buf.get(..n)?);
        self.head = (self.head + 1) % QUEUE;
        self.len -= 1;
        Some(n)
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

/// Both queues and the link state.
#[derive(Debug, Clone)]
pub struct BleLink {
    inbound: Ring,
    outbound: Ring,
    authenticated: bool,
    last_notify_ms: Option<u64>,
}

impl Default for BleLink {
    fn default() -> Self {
        Self::new()
    }
}

impl BleLink {
    pub const fn new() -> Self {
        Self {
            inbound: Ring::new(),
            outbound: Ring::new(),
            authenticated: false,
            last_notify_ms: None,
        }
    }

    /// "Connected" in the C++ sense: paired, or reconnected with a bond (BLE:77-88).
    pub fn is_connected(&self) -> bool {
        self.authenticated
    }

    /// Pairing succeeded, or a bonded peer encrypted the link.
    pub fn on_authenticated(&mut self) {
        self.authenticated = true;
    }

    /// Disconnection or failed pairing: both queues are flushed (BLE:218-226). The node's
    /// own state (offline queue, `app_target_ver`) is untouched.
    pub fn on_disconnect(&mut self) {
        self.authenticated = false;
        self.inbound.clear();
        self.outbound.clear();
    }

    /// A write on RX. Longer than 176 bytes or the queue full: dropped, `false` (BLE:114-129).
    /// An empty write carries no frame.
    pub fn on_write(&mut self, data: &[u8]) -> bool {
        !data.is_empty() && self.inbound.push(data)
    }

    /// Next frame from the app.
    pub fn recv(&mut self, out: &mut [u8; FRAME_MAX]) -> Option<usize> {
        self.inbound.pop(out)
    }

    /// A frame for the app. Dropped (`false`) unless connected, or when the queue is full
    /// (BLE:146-166).
    pub fn send(&mut self, frame: &[u8]) -> bool {
        self.authenticated && !frame.is_empty() && self.outbound.push(frame)
    }

    /// Frames waiting to be notified.
    pub fn pending(&self) -> usize {
        self.outbound.len
    }

    /// When the next notification may go out, if one is waiting.
    pub fn notify_at(&self) -> Option<u64> {
        (self.outbound.len > 0).then(|| {
            self.last_notify_ms
                .map_or(0, |t| t.saturating_add(NOTIFY_GAP_MS))
        })
    }

    /// The frame to notify now, if one is waiting and 60 ms have passed since the last one
    /// (BLE:168-185).
    pub fn next_notify(&mut self, now_ms: u64, out: &mut [u8; FRAME_MAX]) -> Option<usize> {
        if self.notify_at()? > now_ms {
            return None;
        }
        let n = self.outbound.pop(out)?;
        self.last_notify_ms = Some(now_ms);
        Some(n)
    }
}

/// Advertised name: `MeshCore-` and the node name; a node name of `@@MAC` becomes the 12 hex
/// digits of the MAC address, last byte first (BLE:16-24). Returns the length written.
pub fn device_name(node_name: &[u8], mac: [u8; 6], out: &mut [u8; NAME_MAX]) -> usize {
    let hex = |v: u8| if v < 10 { b'0' + v } else { b'A' + v - 10 };
    let mut n = 0;
    let mut put = |b: u8| {
        if let Some(slot) = out.get_mut(n) {
            *slot = b;
            n += 1;
        }
    };
    for &b in NAME_PREFIX {
        put(b);
    }
    if node_name == b"@@MAC" {
        for &b in mac.iter().rev() {
            put(hex(b >> 4));
            put(hex(b & 0x0F));
        }
    } else {
        for &b in node_name.iter().take(31) {
            put(b);
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> BleLink {
        let mut l = BleLink::new();
        l.on_authenticated();
        l
    }

    #[test]
    fn four_frames_each_way_then_drops() {
        let mut l = link();
        for i in 0..4u8 {
            assert!(l.on_write(&[i; 3]));
            assert!(l.send(&[i; 5]));
        }
        assert!(!l.on_write(&[9]), "inbound full");
        assert!(!l.send(&[9]), "outbound full");
        let mut out = [0u8; FRAME_MAX];
        for i in 0..4u8 {
            assert_eq!(l.recv(&mut out), Some(3));
            assert_eq!(out[0], i, "first in, first out");
        }
        assert_eq!(l.recv(&mut out), None);
    }

    #[test]
    fn oversized_and_empty_writes_are_dropped() {
        let mut l = link();
        assert!(!l.on_write(&[0; FRAME_MAX + 1]));
        assert!(!l.on_write(&[]));
        assert!(l.on_write(&[1; FRAME_MAX]), "176 bytes is a frame");
        let mut out = [0u8; FRAME_MAX];
        assert_eq!(l.recv(&mut out), Some(FRAME_MAX));
    }

    #[test]
    fn nothing_goes_out_before_authentication() {
        let mut l = BleLink::new();
        assert!(!l.send(&[1, 2]));
        assert_eq!(l.notify_at(), None);
        l.on_authenticated();
        assert!(l.send(&[1, 2]));
        assert_eq!(l.notify_at(), Some(0));
    }

    #[test]
    fn notifications_are_60_ms_apart() {
        let mut l = link();
        let mut out = [0u8; FRAME_MAX];
        for i in 0..3u8 {
            assert!(l.send(&[i]));
        }
        assert_eq!(l.next_notify(1000, &mut out), Some(1));
        assert_eq!(l.next_notify(1059, &mut out), None, "too soon");
        assert_eq!(l.notify_at(), Some(1060));
        assert_eq!(l.next_notify(1060, &mut out), Some(1));
        assert_eq!(out[0], 1);
        assert_eq!(l.next_notify(2000, &mut out), Some(1));
        assert_eq!(out[0], 2);
        assert_eq!(l.next_notify(3000, &mut out), None, "empty");
        assert_eq!(l.notify_at(), None);
    }

    #[test]
    fn disconnect_flushes_both_queues() {
        let mut l = link();
        assert!(l.on_write(&[1]));
        assert!(l.send(&[2]));
        l.on_disconnect();
        let mut out = [0u8; FRAME_MAX];
        assert_eq!(l.recv(&mut out), None);
        assert_eq!(l.pending(), 0);
        assert!(!l.is_connected());
        assert!(!l.send(&[3]));
    }

    #[test]
    fn ring_wraps_around() {
        let mut l = link();
        let mut out = [0u8; FRAME_MAX];
        for i in 0..10u8 {
            assert!(l.on_write(&[i, i]));
            assert!(l.on_write(&[i, i, i]));
            assert_eq!(l.recv(&mut out), Some(2));
            assert_eq!(l.recv(&mut out), Some(3));
            assert_eq!(out[0], i);
        }
    }

    #[test]
    fn names() {
        let mut out = [0u8; NAME_MAX];
        let n = device_name(b"Tanguy M5", [0; 6], &mut out);
        assert_eq!(&out[..n], b"MeshCore-Tanguy M5");
        let n = device_name(b"@@MAC", [0x24, 0x58, 0x7c, 0xab, 0x01, 0xfe], &mut out);
        assert_eq!(&out[..n], b"MeshCore-FE01AB7C5824");
        let n = device_name(&[b'x'; 40], [0; 6], &mut out);
        assert_eq!(n, NAME_MAX);
    }
}
