//! The text console the C++ firmwares share, as far as the radio settings go
//! (`src/helpers/CommonRadioPrefs.cpp:31-231`, MeshCore, MIT), with the C helpers it relies on:
//! `atoi`, `atof`/`strtof` and MeshCore's `StrHelper::ftoa`/`ftoa3`. Replies are the C++ texts
//! byte for byte. Without floats: values the C++ keeps as `float` are rebuilt as the `float` it
//! would hold (the nearest one), then printed by the same digit algorithm.

use core::fmt::Write;

use crate::node::Prefs;

/// Longest reply (`reply_buf`, minus room for the frame header).
pub const REPLY_MAX: usize = 160;
pub type Reply = heapless::String<REPLY_MAX>;

/// What a radio command changed, for the caller to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Read only.
    None,
    /// Saved; nothing to apply now (radio parameters: "reboot to apply").
    Saved,
    /// Saved, and the transmit power is to be applied now.
    TxPower,
    /// Saved, and the RX gain mode is to be applied now.
    RxGain,
    /// Saved, and used by the firmware from now on (delays, LBT, AGC).
    Live,
}

// --- C library behaviour -------------------------------------------------------------------

/// C `atoi`: leading spaces, a sign, digits up to the first non-digit; 0 if none. Overflow
/// wraps (it is undefined in C).
pub fn atoi(s: &str) -> i32 {
    let b = s
        .trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c'])
        .as_bytes();
    let (neg, digits) = match b {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        _ => (false, b),
    };
    let mut v: i32 = 0;
    for &c in digits.iter().take_while(|c| c.is_ascii_digit()) {
        v = v.wrapping_mul(10).wrapping_add(i32::from(c - b'0'));
    }
    if neg { v.wrapping_neg() } else { v }
}

/// A decimal number like C `strtod`: `Some(value × 10⁶)`, or `None` when no number starts
/// the text (`strtof`'s `end == start`). Digits past the sixth decimal are dropped; exponents
/// (`1e2`) are understood.
pub fn strtod_micro(s: &str) -> Option<i64> {
    let b = s
        .trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c'])
        .as_bytes();
    let mut i = 0;
    let neg = match b.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let mut mant: i128 = 0;
    let mut scale: i32 = 0;
    let mut any = false;
    while let Some(c) = b.get(i).filter(|c| c.is_ascii_digit()) {
        mant = (mant * 10 + i128::from(c - b'0')).min(i128::from(i64::MAX));
        any = true;
        i += 1;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        while let Some(c) = b.get(i).filter(|c| c.is_ascii_digit()) {
            if scale < 18 {
                mant = mant * 10 + i128::from(c - b'0');
                scale += 1;
            }
            any = true;
            i += 1;
        }
    }
    if !any {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let rest = core::str::from_utf8(b.get(i + 1..).unwrap_or(&[])).unwrap_or("");
        if rest
            .trim_start_matches(['+', '-'])
            .starts_with(|c: char| c.is_ascii_digit())
        {
            scale -= atoi(rest).clamp(-40, 40);
        }
    }
    // value × 10⁶ = mant × 10^(6 − scale)
    let shift = 6 - scale;
    let mut v = mant;
    if shift >= 0 {
        for _ in 0..shift {
            v = (v * 10).min(i128::from(i64::MAX));
        }
    } else {
        for _ in 0..-shift {
            v /= 10;
        }
    }
    let v = i64::try_from(v).unwrap_or(i64::MAX);
    Some(if neg { -v } else { v })
}

/// C `atof`: like [`strtod_micro`], 0 when there is no number.
pub fn atof_micro(s: &str) -> i64 {
    strtod_micro(s).unwrap_or(0)
}

/// The IEEE single-precision `float` nearest to `num / den` (round half to even), as bits:
/// what a C `float` holds after `(float)x / 1000.0f` or `strtof`.
pub fn f32_bits(num: i64, den: u64) -> u32 {
    if num == 0 || den == 0 {
        return 0;
    }
    let sign = if num < 0 { 1u32 << 31 } else { 0 };
    let n = u128::from(num.unsigned_abs());
    let d = u128::from(den);
    // Find e with 2^23 ≤ n·2^-e / d < 2^24, i.e. a 24-bit mantissa.
    let mut e: i32 = 0;
    let (mut nn, mut dd) = (n, d);
    while nn < dd << 23 {
        nn <<= 1;
        e -= 1;
    }
    while nn >= dd << 24 {
        dd <<= 1;
        e += 1;
    }
    let q = nn / dd;
    let r = nn % dd;
    let mut m = q;
    if r * 2 > dd || (r * 2 == dd && q & 1 == 1) {
        m += 1;
    }
    if m == 1 << 24 {
        m >>= 1;
        e += 1;
    }
    // value = m × 2^e, m in [2^23, 2^24): exponent field = e + 23 + 127.
    let exp = e + 23 + 127;
    if exp <= 0 {
        return sign;
    }
    if exp >= 255 {
        return sign | 0x7F80_0000;
    }
    sign | ((exp as u32) << 23) | ((m as u32) & 0x7F_FFFF)
}

/// MeshCore's `StrHelper::ftoa` (`_ftoa`, `src/helpers/TxtDataHelpers.cpp:57-146`) on a
/// `float` given by its bits: integer part, a point, up to 7 decimals without trailing zeros
/// (at least one); "0" when out of range.
pub fn ftoa(out: &mut Reply, bits: u32) {
    if bits & 0x7FFF_FFFF == 0 {
        let _ = out.push_str("0.0");
        return;
    }
    let exp2 = i32::from((bits >> 23) as u8) - 127;
    let mantissa = (bits & 0x7F_FFFF) | 0x80_0000;
    let (int_part, mut frac) = if !(-23..31).contains(&exp2) {
        let _ = out.push('0');
        return;
    } else if exp2 >= 23 {
        (u64::from(mantissa) << (exp2 - 23), 0u32)
    } else if exp2 >= 0 {
        (
            u64::from(mantissa >> (23 - exp2)),
            (mantissa << (exp2 + 1)) & 0xFF_FFFF,
        )
    } else {
        (0, (mantissa & 0xFF_FFFF) >> (-(exp2 + 1)))
    };
    if bits & 0x8000_0000 != 0 {
        let _ = out.push('-');
    }
    let _ = write!(out, "{int_part}.");
    if frac == 0 {
        let _ = out.push('0');
        return;
    }
    let mut digits = [0u8; 7];
    for d in &mut digits {
        frac *= 10;
        *d = b'0' + (frac >> 24) as u8;
        frac &= 0xFF_FFFF;
    }
    let keep = digits.iter().rposition(|&d| d != b'0').map_or(1, |i| i + 1);
    for &d in digits.iter().take(keep) {
        let _ = out.push(char::from(d));
    }
}

/// `ftoa` of a value kept in thousandths (frequency in kHz → MHz, factors × 1000).
pub fn ftoa_milli(out: &mut Reply, milli: i64) {
    ftoa(out, f32_bits(milli, 1000));
}

/// MeshCore's `StrHelper::ftoa3`: rounded to 3 decimals, trailing zeros and point dropped.
/// `milli` is the value × 1000, already rounded.
pub fn ftoa3_milli(out: &mut Reply, milli: i64) {
    let whole = milli / 1000;
    let dec = (milli % 1000).unsigned_abs();
    let mut s: heapless::String<24> = heapless::String::new();
    let _ = write!(s, "{whole}.{dec:03}");
    let t = s.trim_end_matches('0').trim_end_matches('.');
    let _ = out.push_str(t);
}

/// `"%d.%d%%"` of `100 / (af + 1)` as `CommonRadioPrefs` computes it (one decimal, rounded half
/// up; the C++ prints "10" when it rounds up to the next unit, and so does this).
fn duty(out: &mut Reply, af_milli: u32) {
    let num = 100_000u64;
    let den = 1000 + u64::from(af_milli);
    let int = num / den;
    let frac = ((num % den) * 20 + den) / (2 * den);
    let _ = write!(out, "{int}.{frac}%");
}

/// `x` in thousandths, from a micro value: rounded to the nearest.
fn milli_of(micro: i64) -> i64 {
    (micro + if micro >= 0 { 500 } else { -500 }) / 1000
}

// --- Radio commands ------------------------------------------------------------------------

/// `CommonRadioPrefs::handleCommand`: `None` when `cmd` is not a radio command.
pub fn radio_command(p: &mut Prefs, cmd: &str, out: &mut Reply) -> Option<Effect> {
    let w = |out: &mut Reply, s: &str| {
        let _ = out.push_str(s);
    };
    if cmd == "get radio" {
        w(out, "> ");
        ftoa_milli(out, i64::from(p.radio.freq_khz));
        w(out, ",");
        ftoa3_milli(out, i64::from(p.radio.bw_hz));
        let _ = write!(out, ",{},{}", p.radio.sf, p.radio.cr);
        return Some(Effect::None);
    }
    if let Some(args) = cmd.strip_prefix("set radio ") {
        let mut parts = args.splitn(4, ',');
        let freq = parts.next().map_or(0, atof_micro);
        let bw = parts.next().map_or(0, atof_micro);
        let sf = parts.next().map_or(0, |s| atoi(s) as u8);
        let cr = parts.next().map_or(0, |s| atoi(s) as u8);
        let ok = (150_000_000..=2_500_000_000).contains(&freq)
            && (5..=12).contains(&sf)
            && (5..=8).contains(&cr)
            && (7_000_000..=500_000_000).contains(&bw);
        if ok {
            p.radio.sf = sf;
            p.radio.cr = cr;
            p.radio.freq_khz = u32::try_from(milli_of(freq)).unwrap_or(p.radio.freq_khz);
            p.radio.bw_hz = u32::try_from(milli_of(bw)).unwrap_or(p.radio.bw_hz);
            w(out, "OK - reboot to apply");
            return Some(Effect::Saved);
        }
        w(out, "Error, invalid radio params");
        return Some(Effect::None);
    }
    if cmd == "get freq" {
        w(out, "> ");
        ftoa_milli(out, i64::from(p.radio.freq_khz));
        return Some(Effect::None);
    }
    if cmd == "get af" {
        w(out, "> ");
        ftoa_milli(out, i64::from(p.airtime_factor_milli));
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set af ") {
        return Some(match strtod_micro(v) {
            Some(af) if (0..=9_000_000).contains(&af) => {
                p.airtime_factor_milli = u32::try_from(milli_of(af)).unwrap_or(0);
                w(out, "OK");
                Effect::Live
            }
            _ => {
                w(out, "ERROR: af must be 0-9");
                Effect::None
            }
        });
    }
    if cmd == "get dutycycle" {
        w(out, "> ");
        duty(out, p.airtime_factor_milli);
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set dutycycle ") {
        let dc = atof_micro(v);
        if !(1_000_000..=100_000_000).contains(&dc) {
            w(out, "ERROR: dutycycle must be 1-100");
            return Some(Effect::None);
        }
        // af = 100 / dc − 1, kept in thousandths.
        let af = (100_000_000_000i64 + dc / 2) / dc - 1000;
        p.airtime_factor_milli = u32::try_from(af).unwrap_or(0);
        w(out, "OK - ");
        duty(out, p.airtime_factor_milli);
        return Some(Effect::Live);
    }
    if cmd == "get int.thresh" {
        let _ = write!(out, "> {}", p.interference_threshold);
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set int.thresh ") {
        p.interference_threshold = atoi(v) as u8;
        w(out, "OK");
        return Some(Effect::Live);
    }
    if cmd == "get cad" {
        w(out, if p.cad_enabled { "> on" } else { "> off" });
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set cad ") {
        p.cad_enabled = v.starts_with("on");
        w(out, "OK");
        return Some(Effect::Live);
    }
    if cmd == "get radio.rxgain" {
        w(out, if p.rx_boosted_gain { "> on" } else { "> off" });
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set radio.rxgain ") {
        p.rx_boosted_gain = v.starts_with("on");
        w(out, "OK");
        return Some(Effect::RxGain);
    }
    if let Some(rest) = cmd.strip_prefix("get tx")
        && (rest.is_empty() || rest.starts_with(' '))
    {
        // `getTxPower` returns a `uint8_t`: −9 dBm prints as 247.
        let _ = write!(out, "> {}", p.radio.tx_power_dbm as u8);
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set tx ") {
        p.radio.tx_power_dbm = atoi(v) as u8 as i8;
        w(out, "OK");
        return Some(Effect::TxPower);
    }
    if cmd == "get rxdelay" {
        w(out, "> ");
        ftoa_milli(out, i64::from(p.rx_delay_base_milli));
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set rxdelay ") {
        let d = atof_micro(v);
        if (0..=20_000_000).contains(&d) {
            p.rx_delay_base_milli = u32::try_from(milli_of(d)).unwrap_or(0);
            w(out, "OK");
            return Some(Effect::Live);
        }
        w(out, "Error, must be 0-20");
        return Some(Effect::None);
    }
    if cmd == "get agc.reset.interval" {
        let _ = write!(out, "> {}", u32::from(p.agc_reset_interval) * 4);
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set agc.reset.interval ") {
        let secs = atoi(v).clamp(0, 255 * 4);
        p.agc_reset_interval = (secs / 4) as u8;
        let _ = write!(
            out,
            "OK - interval rounded to {}",
            u32::from(p.agc_reset_interval) * 4
        );
        return Some(Effect::Live);
    }
    if cmd == "get path.hash.mode" {
        let _ = write!(out, "> {}", p.path_hash_mode);
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set path.hash.mode ") {
        let mode = atoi(v) as u8;
        if mode < 3 {
            p.path_hash_mode = mode;
            w(out, "OK");
            return Some(Effect::Live);
        }
        w(out, "Error, must be 0,1, or 2");
        return Some(Effect::None);
    }
    if cmd == "get multi.acks" {
        let _ = write!(out, "> {}", p.multi_acks);
        return Some(Effect::None);
    }
    if let Some(v) = cmd.strip_prefix("set multi.acks ") {
        p.multi_acks = atoi(v) as u8;
        w(out, "OK");
        return Some(Effect::Live);
    }
    for (key, field) in [
        ("txdelay", &mut p.tx_delay_milli),
        ("direct.txdelay", &mut p.direct_tx_delay_milli),
    ] {
        if cmd.strip_prefix("get ") == Some(key) {
            w(out, "> ");
            ftoa_milli(out, i64::from(*field));
            return Some(Effect::None);
        }
        if let Some(v) = cmd
            .strip_prefix("set ")
            .and_then(|r| r.strip_prefix(key))
            .and_then(|r| r.strip_prefix(' '))
        {
            let f = atof_micro(v);
            if (0..=2_000_000).contains(&f) {
                *field = u32::try_from(milli_of(f)).unwrap_or(0);
                w(out, "OK");
                return Some(Effect::Live);
            }
            w(out, "Error, must be 0-2");
            return Some(Effect::None);
        }
    }
    None
}

/// `AdvertDataParser::isValidName`.
pub fn valid_name(name: &[u8]) -> bool {
    !name
        .iter()
        .any(|c| matches!(c, b'[' | b']' | b'\\' | b':' | b',' | b'?' | b'*'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(p: &mut Prefs, cmd: &str) -> (Option<Effect>, Reply) {
        let mut out = Reply::new();
        let e = radio_command(p, cmd, &mut out);
        (e, out)
    }

    fn ftoa_s(bits: u32) -> Reply {
        let mut o = Reply::new();
        ftoa(&mut o, bits);
        o
    }

    #[test]
    fn floats_like_c() {
        // Bit patterns of the C `float`s (IEEE single precision).
        const F869_618: u32 = 0x4459_678D;
        assert_eq!(f32_bits(869_618, 1000), F869_618); // (float)869618 / 1000.0f
        assert_eq!(f32_bits(1, 1), 0x3F80_0000);
        assert_eq!(f32_bits(500, 1000), 0x3F00_0000);
        assert_eq!(f32_bits(200, 1000), 0x3E4C_CCCD);
        assert_eq!(f32_bits(-9, 1), 0xC110_0000);
        assert_eq!(f32_bits(433_175, 1000), 0x43D8_9666);
        assert_eq!(f32_bits(1, 3), 0x3EAA_AAAB);
        assert_eq!(ftoa_s(F869_618), "869.6179809");
        assert_eq!(ftoa_s(0x3F80_0000), "1.0");
        assert_eq!(ftoa_s(0x3F00_0000), "0.5");
        // The C++ shifts low mantissa bits out before printing: 0.2f comes out as 0.1999999.
        assert_eq!(ftoa_s(0x3E4C_CCCD), "0.1999999");
        assert_eq!(ftoa_s(0), "0.0");
        assert_eq!(ftoa_s(0xC010_0000), "-2.25");
        let mut o = Reply::new();
        ftoa3_milli(&mut o, 62_500);
        assert_eq!(o, "62.5");
        o.clear();
        ftoa3_milli(&mut o, 250_000);
        assert_eq!(o, "250");
        o.clear();
        ftoa3_milli(&mut o, 31_250);
        assert_eq!(o, "31.25");
    }

    #[test]
    fn c_parsers() {
        assert_eq!(atoi("  -12abc"), -12);
        assert_eq!(atoi("x"), 0);
        assert_eq!(atoi("+7"), 7);
        assert_eq!(strtod_micro("869.618"), Some(869_618_000));
        assert_eq!(strtod_micro(" 0.5"), Some(500_000));
        assert_eq!(strtod_micro("-1.25e1"), Some(-12_500_000));
        assert_eq!(strtod_micro(".5"), Some(500_000));
        assert_eq!(strtod_micro("abc"), None);
        assert_eq!(atof_micro("abc"), 0);
    }

    #[test]
    fn radio_replies() {
        let mut p = Prefs::defaults(&[1; 32]);
        p.radio.freq_khz = 869_618;
        p.radio.bw_hz = 62_500;
        p.radio.sf = 8;
        p.radio.cr = 8;
        assert_eq!(run(&mut p, "get radio").1, "> 869.6179809,62.5,8,8");
        assert_eq!(run(&mut p, "get freq").1, "> 869.6179809");
        let (e, r) = run(&mut p, "set radio 869.525,250,11,5");
        assert_eq!(
            (e, r.as_str()),
            (Some(Effect::Saved), "OK - reboot to apply")
        );
        assert_eq!(
            (p.radio.freq_khz, p.radio.bw_hz, p.radio.sf, p.radio.cr),
            (869_525, 250_000, 11, 5)
        );
        assert_eq!(run(&mut p, "get radio").1, "> 869.5250244,250,11,5");
        assert_eq!(
            run(&mut p, "set radio 869.5,250,13,5").1,
            "Error, invalid radio params"
        );
        p.airtime_factor_milli = 1000;
        assert_eq!(run(&mut p, "get af").1, "> 1.0");
        assert_eq!(run(&mut p, "get dutycycle").1, "> 50.0%");
        assert_eq!(run(&mut p, "set dutycycle 10").1, "OK - 10.0%");
        assert_eq!(p.airtime_factor_milli, 9000);
        assert_eq!(
            run(&mut p, "set dutycycle 0.5").1,
            "ERROR: dutycycle must be 1-100"
        );
        assert_eq!(run(&mut p, "set af x").1, "ERROR: af must be 0-9");
        assert_eq!(run(&mut p, "set af 9.5").1, "ERROR: af must be 0-9");
        assert_eq!(run(&mut p, "set af 2").1, "OK");
        assert_eq!(run(&mut p, "get dutycycle").1, "> 33.3%");
        p.radio.tx_power_dbm = -9;
        assert_eq!(run(&mut p, "get tx").1, "> 247");
        assert_eq!(run(&mut p, "get tx please").1, "> 247");
        assert_eq!(run(&mut p, "get txx"), (None, Reply::new()));
        assert_eq!(run(&mut p, "set tx 14").0, Some(Effect::TxPower));
        assert_eq!(p.radio.tx_power_dbm, 14);
        assert_eq!(
            run(&mut p, "set agc.reset.interval 30").1,
            "OK - interval rounded to 28"
        );
        assert_eq!(run(&mut p, "get agc.reset.interval").1, "> 28");
        assert_eq!(
            run(&mut p, "set agc.reset.interval 5000").1,
            "OK - interval rounded to 1020"
        );
        assert_eq!(run(&mut p, "set cad onward").1, "OK");
        assert_eq!(run(&mut p, "get cad").1, "> on");
        assert_eq!(run(&mut p, "set radio.rxgain off").0, Some(Effect::RxGain));
        assert_eq!(run(&mut p, "get radio.rxgain").1, "> off");
        assert_eq!(run(&mut p, "set int.thresh 300").1, "OK");
        assert_eq!(run(&mut p, "get int.thresh").1, "> 44");
        assert_eq!(
            run(&mut p, "set path.hash.mode 3").1,
            "Error, must be 0,1, or 2"
        );
        assert_eq!(run(&mut p, "set rxdelay 21").1, "Error, must be 0-20");
        assert_eq!(run(&mut p, "set txdelay 0.7").1, "OK");
        assert_eq!(run(&mut p, "get txdelay").1, "> 0.6999999");
        assert_eq!(run(&mut p, "set direct.txdelay 3").1, "Error, must be 0-2");
        assert_eq!(run(&mut p, "get direct.txdelay").1, "> 0.1999999");
        assert_eq!(run(&mut p, "reboot"), (None, Reply::new()));
    }
}
