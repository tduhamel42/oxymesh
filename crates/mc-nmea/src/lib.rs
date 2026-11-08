//! GPS NMEA 0183, the part the ThinkNode M5's receiver (L76K) is used for: `RMC` (time, date,
//! validity, position) and `GGA` (fix, satellites, altitude), from any talker (`GP`, `GN`,
//! `BD`…). Positions in degrees × 1e6 like the MeshCore protocol, altitude in millimetres, all
//! in integers. No allocation; never panics on garbage from the UART.
//!
//! [`Gps`] keeps the latest state the way the C++ firmware's `MicroNMEALocationProvider` reads
//! it, and [`TimeSync`] is its clock synchronisation (`MicroNMEALocationProvider.h:53, 232-257`,
//! MeshCore, MIT).
#![no_std]
#![deny(clippy::float_arithmetic)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use mc_core::civil::Civil;

/// Longest sentence kept (82 by the standard, some receivers go a little over).
pub const LINE_MAX: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Time {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

/// Degrees × 1e6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub lat: i32,
    pub lon: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sentence {
    /// Recommended minimum: `valid` is the status `A`.
    Rmc {
        time: Option<Time>,
        valid: bool,
        pos: Option<Position>,
        date: Option<Date>,
    },
    /// Fix data: `fix` 0 is no fix.
    Gga {
        time: Option<Time>,
        pos: Option<Position>,
        fix: u8,
        satellites: u8,
        alt_mm: Option<i32>,
    },
}

/// Parses one sentence, `$` to the checksum (line ending optional). `None` for a bad checksum,
/// another sentence type or a malformed field that matters.
pub fn parse(line: &[u8]) -> Option<Sentence> {
    let line = trim_end(line);
    let body = line.strip_prefix(b"$")?;
    let star = body.iter().rposition(|&b| b == b'*')?;
    let (payload, sum) = (body.get(..star)?, body.get(star + 1..)?);
    if sum.len() != 2 || payload.iter().fold(0u8, |a, b| a ^ b) != hex2(sum)? {
        return None;
    }
    let mut f = payload.split(|&b| b == b',');
    let id = f.next()?;
    if id.len() != 5 {
        return None;
    }
    match id.get(2..)? {
        b"RMC" => {
            let time = f.next().and_then(time);
            let valid = f.next()? == b"A";
            let pos = position(f.next()?, f.next()?, f.next()?, f.next()?);
            let _speed = f.next();
            let _course = f.next();
            let date = f.next().and_then(date);
            Some(Sentence::Rmc {
                time,
                valid,
                pos,
                date,
            })
        }
        b"GGA" => {
            let time = f.next().and_then(time);
            let pos = position(f.next()?, f.next()?, f.next()?, f.next()?);
            let fix = digits(f.next()?).and_then(|v| u8::try_from(v).ok())?;
            let satellites = digits(f.next()?)
                .and_then(|v| u8::try_from(v).ok())
                .unwrap_or(0);
            let _hdop = f.next();
            let alt_mm = f.next().and_then(milli);
            Some(Sentence::Gga {
                time,
                pos,
                fix,
                satellites,
                alt_mm,
            })
        }
        _ => None,
    }
}

fn trim_end(mut s: &[u8]) -> &[u8] {
    while let [rest @ .., b'\r' | b'\n'] = s {
        s = rest;
    }
    s
}

fn hex2(s: &[u8]) -> Option<u8> {
    let v = |c: u8| (c as char).to_digit(16);
    let (a, b) = (v(*s.first()?)?, v(*s.get(1)?)?);
    u8::try_from(a * 16 + b).ok()
}

/// A run of ASCII digits, at most 9 of them.
fn digits(s: &[u8]) -> Option<u32> {
    if s.is_empty() || s.len() > 9 || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(s.iter().fold(0, |a, &c| a * 10 + u32::from(c - b'0')))
}

/// `int.frac` scaled to millionths (fraction cut after 6 digits).
fn micro(s: &[u8]) -> Option<u64> {
    let (int, frac) = match s.iter().position(|&b| b == b'.') {
        Some(i) => (s.get(..i)?, s.get(i + 1..)?),
        None => (s, &b""[..]),
    };
    let int = if int.is_empty() { 0 } else { digits(int)? };
    let mut f: u64 = 0;
    for k in 0..6 {
        let d = match frac.get(k) {
            Some(c) if c.is_ascii_digit() => u64::from(c - b'0'),
            Some(_) => return None,
            None => 0,
        };
        f = f * 10 + d;
    }
    if frac.iter().any(|c| !c.is_ascii_digit()) {
        return None;
    }
    Some(u64::from(int) * 1_000_000 + f)
}

/// A signed decimal (altitude, metres) in thousandths.
fn milli(s: &[u8]) -> Option<i32> {
    let (neg, s) = match s.strip_prefix(b"-") {
        Some(r) => (true, r),
        None => (false, s),
    };
    let v = i64::try_from(micro(s)? / 1000).ok()?;
    i32::try_from(if neg { -v } else { v }).ok()
}

fn time(s: &[u8]) -> Option<Time> {
    let hh = digits(s.get(0..2)?)?;
    let mm = digits(s.get(2..4)?)?;
    let ss = digits(s.get(4..6)?)?;
    (hh < 24 && mm < 60 && ss < 61).then(|| Time {
        hour: hh as u8,
        minute: mm as u8,
        second: ss.min(59) as u8,
    })
}

fn date(s: &[u8]) -> Option<Date> {
    if s.len() != 6 {
        return None;
    }
    let dd = digits(s.get(0..2)?)?;
    let mm = digits(s.get(2..4)?)?;
    let yy = digits(s.get(4..6)?)?;
    ((1..=31).contains(&dd) && (1..=12).contains(&mm)).then(|| Date {
        year: 2000 + yy as u16,
        month: mm as u8,
        day: dd as u8,
    })
}

/// `ddmm.mmmm,N,dddmm.mmmm,E` to degrees × 1e6; `None` when empty or out of range.
fn position(lat: &[u8], ns: &[u8], lon: &[u8], ew: &[u8]) -> Option<Position> {
    let lat = degrees(lat, 2)?;
    let lon = degrees(lon, 3)?;
    let lat = match ns {
        b"N" => lat,
        b"S" => -lat,
        _ => return None,
    };
    let lon = match ew {
        b"E" => lon,
        b"W" => -lon,
        _ => return None,
    };
    (lat.abs() <= 90_000_000 && lon.abs() <= 180_000_000).then_some(Position { lat, lon })
}

/// Degrees then minutes (`deg_digits` of degrees) to degrees × 1e6, truncated.
fn degrees(s: &[u8], deg_digits: usize) -> Option<i32> {
    let deg = digits(s.get(..deg_digits)?)?;
    let min_micro = micro(s.get(deg_digits..)?)?;
    if min_micro >= 60_000_000 {
        return None;
    }
    i32::try_from(u64::from(deg) * 1_000_000 + min_micro / 60).ok()
}

/// Collects UART bytes into sentences.
#[derive(Debug, Clone)]
pub struct Lines {
    buf: [u8; LINE_MAX],
    len: usize,
    /// The current line is too long: dropped up to its end.
    skip: bool,
}

impl Default for Lines {
    fn default() -> Self {
        Self::new()
    }
}

impl Lines {
    pub const fn new() -> Self {
        Self {
            buf: [0; LINE_MAX],
            len: 0,
            skip: false,
        }
    }

    /// One byte; a sentence when it ends a valid line.
    pub fn push(&mut self, b: u8) -> Option<Sentence> {
        match b {
            b'\r' | b'\n' | 0 => {
                let line = self.buf.get(..self.len).filter(|_| !self.skip);
                let s = line.and_then(parse);
                self.len = 0;
                self.skip = false;
                s
            }
            b'$' => {
                // A new sentence, whatever came before.
                self.buf = [0; LINE_MAX];
                self.buf[0] = b'$';
                self.len = 1;
                self.skip = false;
                None
            }
            _ => {
                match self.buf.get_mut(self.len) {
                    Some(slot) => {
                        *slot = b;
                        self.len += 1;
                    }
                    None => self.skip = true,
                }
                None
            }
        }
    }
}

/// What the receiver last said, as `MicroNMEA` keeps it: the validity of the last RMC or GGA,
/// the last position, altitude and satellite count, the last date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Gps {
    pub valid: bool,
    pub pos: Option<Position>,
    pub alt_mm: i32,
    pub satellites: u8,
    pub date: Option<Date>,
    pub time: Option<Time>,
}

impl Gps {
    pub fn apply(&mut self, s: &Sentence) {
        match *s {
            Sentence::Rmc {
                time,
                valid,
                pos,
                date,
            } => {
                self.valid = valid;
                if valid {
                    self.pos = pos.or(self.pos);
                }
                self.time = time.or(self.time);
                self.date = date.or(self.date);
            }
            Sentence::Gga {
                time,
                pos,
                fix,
                satellites,
                alt_mm,
            } => {
                self.valid = fix > 0;
                self.satellites = satellites;
                if fix > 0 {
                    self.pos = pos.or(self.pos);
                    self.alt_mm = alt_mm.unwrap_or(self.alt_mm);
                }
                self.time = time.or(self.time);
            }
        }
    }

    /// The receiver's UTC time, once it knows the date.
    pub fn unix(&self) -> Option<u32> {
        let (d, t) = (self.date?, self.time?);
        Civil {
            year: i32::from(d.year),
            month: d.month,
            day: d.day,
            hour: t.hour,
            minute: t.minute,
            second: t.second,
            weekday: 0,
        }
        .to_unix()
    }
}

/// The C++ firmware's clock sync from the GPS: once a second, count the seconds with a valid
/// fix; after more than two in a row, set the clock if a sync is due. Due at start, then every
/// 30 minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSync {
    valid_secs: u32,
    needed: bool,
    last_sync_ms: u64,
    next_check_ms: u64,
}

/// Re-sync every (`TIME_SYNC_INTERVAL`), ms.
pub const RESYNC_MS: u64 = 1_800_000;

impl Default for TimeSync {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeSync {
    pub const fn new() -> Self {
        Self {
            valid_secs: 0,
            needed: true,
            last_sync_ms: 0,
            next_check_ms: 0,
        }
    }

    /// Call often; the time to set the clock to, when it is due.
    pub fn poll(&mut self, now_ms: u64, gps: &Gps) -> Option<u32> {
        if !gps.valid {
            self.valid_secs = 0;
        }
        if now_ms < self.next_check_ms {
            return None;
        }
        self.next_check_ms = now_ms + 1000;
        if !self.needed && now_ms.saturating_sub(self.last_sync_ms) > RESYNC_MS {
            self.needed = true;
        }
        let mut out = None;
        if self.needed
            && self.valid_secs > 2
            && let Some(t) = gps.unix()
        {
            out = Some(t);
            self.needed = false;
            self.last_sync_ms = now_ms;
        }
        if gps.valid {
            self.valid_secs += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sentence with its checksum.
    fn s(payload: &str) -> std::string::String {
        let sum = payload.bytes().fold(0u8, |a, b| a ^ b);
        std::format!("${payload}*{sum:02X}\r\n")
    }

    extern crate std;

    // Made-up places: none of these is anyone's home.
    const RMC: &str = "GNRMC,123519.00,A,4807.038,N,01131.000,E,0.02,,230394,,,A";
    const GGA: &str = "GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,";

    #[test]
    fn rmc_and_gga() {
        assert_eq!(
            parse(s(RMC).as_bytes()),
            Some(Sentence::Rmc {
                time: Some(Time {
                    hour: 12,
                    minute: 35,
                    second: 19
                }),
                valid: true,
                pos: Some(Position {
                    lat: 48_117_300,
                    lon: 11_516_666
                }),
                date: Some(Date {
                    year: 2094,
                    month: 3,
                    day: 23
                }),
            })
        );
        assert_eq!(
            parse(s(GGA).as_bytes()),
            Some(Sentence::Gga {
                time: Some(Time {
                    hour: 12,
                    minute: 35,
                    second: 19
                }),
                pos: Some(Position {
                    lat: 48_117_300,
                    lon: 11_516_666
                }),
                fix: 1,
                satellites: 8,
                alt_mm: Some(545_400),
            })
        );
        // South and west, a negative altitude.
        let g = parse(s("GPGGA,000000,3351.000,S,07037.500,W,2,12,1.0,-12.5,M,,M,,").as_bytes());
        assert!(matches!(
            g,
            Some(Sentence::Gga {
                pos: Some(Position {
                    lat: -33_850_000,
                    lon: -70_625_000
                }),
                alt_mm: Some(-12_500),
                ..
            })
        ));
    }

    #[test]
    fn no_fix_yet() {
        let r = parse(s("GPRMC,,V,,,,,,,,,,N").as_bytes());
        assert_eq!(
            r,
            Some(Sentence::Rmc {
                time: None,
                valid: false,
                pos: None,
                date: None
            })
        );
        let g = parse(s("GPGGA,,,,,,0,00,99.99,,,,,,").as_bytes());
        assert!(matches!(
            g,
            Some(Sentence::Gga {
                fix: 0,
                pos: None,
                ..
            })
        ));
    }

    #[test]
    fn rejects() {
        let good = s(RMC);
        let mut bad = good.clone().into_bytes();
        bad[10] ^= 1;
        assert_eq!(parse(&bad), None, "checksum");
        assert_eq!(parse(s("GPGSV,3,1,11").as_bytes()), None, "other sentence");
        assert_eq!(parse(b"GPRMC,1,A*00"), None, "no $");
        assert_eq!(
            parse(s("GPRMC,123519,A,4860.000,N,01131.000,E,,,230394,,").as_bytes()).and_then(|r| {
                match r {
                    Sentence::Rmc { pos, .. } => pos,
                    _ => None,
                }
            }),
            None,
            "60 minutes"
        );
        assert_eq!(parse(b"$*00"), None);
        assert_eq!(parse(b""), None);
    }

    #[test]
    fn lines_from_a_noisy_uart() {
        let mut l = Lines::new();
        let mut out = std::vec::Vec::new();
        let stream = std::format!(
            "garbage{}$GPGSV,1*00\r\n{}{}",
            s(RMC),
            s(GGA),
            "x".repeat(300)
        );
        for b in stream.bytes() {
            out.extend(l.push(b));
        }
        out.extend(l.push(b'\n'));
        assert_eq!(out.len(), 2);
        let mut gps = Gps::default();
        for x in &out {
            gps.apply(x);
        }
        assert!(gps.valid);
        assert_eq!(gps.satellites, 8);
        assert_eq!(gps.alt_mm, 545_400);
        // 2094-03-23 12:35:19 UTC.
        assert_eq!(gps.unix(), Some(3_920_186_119));
    }

    #[test]
    fn clock_sync_like_the_cpp() {
        let mut gps = Gps::default();
        gps.apply(&parse(s(RMC).as_bytes()).unwrap());
        let mut ts = TimeSync::new();
        let mut set = std::vec::Vec::new();
        for sec in 0..10u64 {
            if let Some(t) = ts.poll(sec * 1000, &gps) {
                set.push(sec);
                let _ = t;
            }
        }
        // Valid seconds counted at 0, 1, 2; more than two at the check of second 3.
        assert_eq!(set, [3]);
        // Lost fix: the count starts again; the next sync is 30 minutes later.
        gps.valid = false;
        assert_eq!(ts.poll(10_000, &gps), None);
        gps.valid = true;
        let mut set = std::vec::Vec::new();
        for sec in 11..2000u64 {
            if ts.poll(sec * 1000, &gps).is_some() {
                set.push(sec);
            }
        }
        assert_eq!(set, [1804]);
    }
}
