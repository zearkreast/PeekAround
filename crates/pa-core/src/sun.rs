//! Solar position (mirrors `sun.ts`) — the low-precision NOAA / suncalc algorithm.

use serde::{Deserialize, Serialize};

const RAD: f64 = std::f64::consts::PI / 180.0;
const DAY_MS: f64 = 86_400_000.0;
/// 2000-01-01T12:00:00Z
const J2000_MS: f64 = 946_728_000_000.0;
const OBLIQUITY: f64 = 23.4397 * RAD;

/// sun altitude above the horizon in degrees (negative = below) at a UTC instant and place
pub fn sun_altitude(instant_ms: f64, lat: f64, lon: f64) -> f64 {
    let d = (instant_ms - J2000_MS) / DAY_MS;
    let m = (357.5291 + 0.98560028 * d) * RAD;
    let c = (1.9148 * m.sin() + 0.02 * (2.0 * m).sin() + 0.0003 * (3.0 * m).sin()) * RAD;
    let l = m + c + 102.9372 * RAD + std::f64::consts::PI;
    let dec = (OBLIQUITY.sin() * l.sin()).asin();
    let ra = (l.sin() * OBLIQUITY.cos()).atan2(l.cos());
    let sidereal = (280.16 + 360.9856235 * d) * RAD + lon * RAD;
    let h = sidereal - ra;
    let phi = lat * RAD;
    let alt = (phi.sin() * dec.sin() + phi.cos() * dec.cos() * h.cos()).asin();
    alt / RAD
}

/// golden hour: sun between 4° below and 6° above the horizon
pub const GOLDEN_MIN: f64 = -4.0;
/// see [`GOLDEN_MIN`]
pub const GOLDEN_MAX: f64 = 6.0;
/// "night" once the sun is more than 8° below the horizon
pub const NIGHT_MAX: f64 = -8.0;

/// Time of day by sun altitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeOfDay {
    /// night
    Night,
    /// golden hour
    Golden,
    /// day
    Day,
    /// twilight
    Twilight,
}

/// time of day of a sun altitude
pub fn time_of_day_of(altitude: f64) -> TimeOfDay {
    if altitude < NIGHT_MAX {
        TimeOfDay::Night
    } else if altitude < GOLDEN_MIN {
        TimeOfDay::Twilight
    } else if altitude <= GOLDEN_MAX {
        TimeOfDay::Golden
    } else {
        TimeOfDay::Day
    }
}

/// wall-clock hour that counts as night when the sun position is unknown: 22:00–04:59
pub fn is_night_hour(hour: i64) -> bool {
    !(5..22).contains(&hour)
}

/// Best guess of the capture instant for a wall-clock-only timestamp at a known place.
pub fn estimate_instant(local_at: i64, home_instant: i64, lon: f64) -> i64 {
    let home_off = crate::js::round((local_at - home_instant) as f64 / 60_000.0);
    let lon_off = crate::js::round(lon / 15.0) * 60.0;
    if (home_off - lon_off).abs() <= 90.0 {
        return home_instant;
    }
    local_at - (lon_off * 60_000.0) as i64
}
