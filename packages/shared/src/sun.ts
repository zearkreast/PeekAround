/**
 * Solar position (pure) — the low-precision algorithm used by NOAA / suncalc
 * (accurate to a few tenths of a degree between 1950 and 2050, plenty for "golden hour").
 */

const RAD = Math.PI / 180;
const DAY_MS = 86_400_000;
/** 2000-01-01T12:00:00Z */
const J2000_MS = 946_728_000_000;
const OBLIQUITY = 23.4397 * RAD;

/** sun altitude above the horizon in degrees (negative = below) at a UTC instant and place */
export function sunAltitude(instantMs: number, lat: number, lon: number): number {
  const d = (instantMs - J2000_MS) / DAY_MS;
  const M = (357.5291 + 0.98560028 * d) * RAD; // mean anomaly
  const C = (1.9148 * Math.sin(M) + 0.02 * Math.sin(2 * M) + 0.0003 * Math.sin(3 * M)) * RAD; // equation of center
  const L = M + C + 102.9372 * RAD + Math.PI; // ecliptic longitude
  const dec = Math.asin(Math.sin(OBLIQUITY) * Math.sin(L));
  const ra = Math.atan2(Math.sin(L) * Math.cos(OBLIQUITY), Math.cos(L));
  const sidereal = (280.16 + 360.9856235 * d) * RAD + lon * RAD;
  const H = sidereal - ra; // hour angle
  const phi = lat * RAD;
  const alt = Math.asin(Math.sin(phi) * Math.sin(dec) + Math.cos(phi) * Math.cos(dec) * Math.cos(H));
  return alt / RAD;
}

/** golden hour: sun between 4° below and 6° above the horizon */
export const GOLDEN_MIN = -4;
export const GOLDEN_MAX = 6;
/** "night" once the sun is more than 8° below the horizon (past civil twilight) */
export const NIGHT_MAX = -8;

export type TimeOfDay = 'night' | 'golden' | 'day' | 'twilight';

export function timeOfDayOf(altitude: number): TimeOfDay {
  if (altitude < NIGHT_MAX) return 'night';
  if (altitude < GOLDEN_MIN) return 'twilight';
  if (altitude <= GOLDEN_MAX) return 'golden';
  return 'day';
}

/** wall-clock hour (localAt) that counts as night when the sun position is unknown: 22:00–04:59 */
export function isNightHour(hour: number): boolean {
  return hour >= 22 || hour < 5;
}

/**
 * Best guess of the capture instant for a wall-clock-only timestamp (no EXIF offset) at a known place.
 * The home-zone instant is right while at home; abroad, a clock read in the home zone can be hours
 * off — then the offset implied by the longitude (solar time, ±1 h for most zones) is closer.
 * @param homeInstant instant resolved with the home time zone
 */
export function estimateInstant(localAt: number, homeInstant: number, lon: number): number {
  const homeOff = Math.round((localAt - homeInstant) / 60_000);
  const lonOff = Math.round(lon / 15) * 60;
  if (Math.abs(homeOff - lonOff) <= 90) return homeInstant;
  return localAt - lonOff * 60_000;
}
