//! Distance and bearing between two positions (degrees × 1e6), on a spherical Earth: plenty
//! for a 200-pixel screen.

use alloc::format;
use alloc::string::String;

const EARTH_M: f32 = 6_371_000.0;

fn rad(micro_deg: i32) -> f32 {
    (micro_deg as f32 / 1e6).to_radians()
}

/// Meters between two points (haversine).
pub fn distance_m(a: (i32, i32), b: (i32, i32)) -> f32 {
    let (la1, lo1, la2, lo2) = (rad(a.0), rad(a.1), rad(b.0), rad(b.1));
    let s1 = libm::sinf((la2 - la1) / 2.0);
    let s2 = libm::sinf((lo2 - lo1) / 2.0);
    let h = s1 * s1 + libm::cosf(la1) * libm::cosf(la2) * s2 * s2;
    2.0 * EARTH_M * libm::asinf(libm::sqrtf(h.clamp(0.0, 1.0)))
}

/// Initial bearing from `a` to `b`, degrees from north, 0 to 359.
pub fn bearing_deg(a: (i32, i32), b: (i32, i32)) -> u16 {
    let (la1, lo1, la2, lo2) = (rad(a.0), rad(a.1), rad(b.0), rad(b.1));
    let y = libm::sinf(lo2 - lo1) * libm::cosf(la2);
    let x = libm::cosf(la1) * libm::sinf(la2)
        - libm::sinf(la1) * libm::cosf(la2) * libm::cosf(lo2 - lo1);
    let d = libm::atan2f(y, x).to_degrees();
    ((d + 360.0) % 360.0) as u16
}

/// `350 m`, `4.2 km`, `37 km`.
pub fn distance_text(m: f32) -> String {
    if m < 1000.0 {
        format!("{} m", m as u32)
    } else if m < 10_000.0 {
        let tenths = (m / 100.0) as u32;
        format!("{}.{} km", tenths / 10, tenths % 10)
    } else {
        format!("{} km", (m / 1000.0) as u32)
    }
}

/// Compass point of a bearing: N, NE, E…
pub fn compass(deg: u16) -> &'static str {
    const P: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    P[usize::from((deg + 22) % 360 / 45) % 8]
}

/// `45.123456 N`, `6.5 E` style: degrees with up to 4 decimals and a hemisphere.
pub fn coord(micro: i32, lat: bool) -> String {
    let hemi = match (lat, micro >= 0) {
        (true, true) => "N",
        (true, false) => "S",
        (false, true) => "E",
        (false, false) => "W",
    };
    let a = micro.unsigned_abs();
    format!("{}.{:04} {hemi}", a / 1_000_000, a % 1_000_000 / 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paris_to_lyon() {
        let paris = (48_856_600, 2_352_200);
        let lyon = (45_764_000, 4_835_700);
        let d = distance_m(paris, lyon);
        assert!((390_000.0..395_000.0).contains(&d), "{d}");
        let b = bearing_deg(paris, lyon);
        assert!((150..160).contains(&b), "{b}");
        assert_eq!(compass(b), "SE");
        assert_eq!(distance_text(350.4), "350 m");
        assert_eq!(distance_text(4_250.0), "4.2 km");
        assert_eq!(coord(-45_123_456, true), "45.1234 S");
    }
}
