/**
 * Capture-time helpers (pure, environment agnostic).
 *
 * Two clocks are kept for every item:
 *  - instant ("takenAt"): true UTC ms epoch
 *  - local ("localAt"): wall-clock time at the place of capture, encoded as if it were UTC.
 *    Group / label it with UTC getters (`getUTCDate()`, `timeZone: 'UTC'`).
 *
 * Offsets are minutes EAST of UTC (Istanbul = +180).
 */

const MIN = 60_000;
const DAY = 86_400_000;

/**
 * Parse an EXIF / ISO-like date-time string into wall-clock ms (as UTC).
 * Accepts "2024:07:01 10:00:00", "2024-07-01T10:00:00.123", "2024:07:01 10:00:00+03:00" (offset ignored here,
 * see parseDateTimeWithOffset) and date-only "2024:07:01".
 */
export function parseExifDateTime(s: unknown): number | null {
  const r = parseDateTimeWithOffset(s);
  return r ? r.local : null;
}

/** Like parseExifDateTime but also returns a trailing offset ("Z", "+03:00", "+0300") when present. */
export function parseDateTimeWithOffset(s: unknown): { local: number; offset: number | null } | null {
  if (typeof s !== 'string') return null;
  // date strings are short; the cap keeps the regex below linear on hostile metadata (ReDoS)
  const str = s.trim();
  if (str.length > 64) return null;
  const m = /^\s*(\d{4})[:\-/](\d{1,2})[:\-/](\d{1,2})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2})(?:[.,](\d+))?)?)?\s*(Z|[+-]\d{2}:?\d{2})?\s*$/i.exec(str);
  if (!m) return null;
  const [y, mo, d, h = '0', mi = '0', se = '0'] = [m[1], m[2], m[3], m[4], m[5], m[6]];
  const Y = Number(y);
  const M = Number(mo);
  const D = Number(d);
  if (Y < 1971 || Y > 2200 || M < 1 || M > 12 || D < 1 || D > 31) return null; // "0000:00:00 00:00:00" etc.
  const ms = m[7] ? Math.round(Number(`0.${m[7]}`) * 1000) : 0;
  const local = Date.UTC(Y, M - 1, D, Number(h), Number(mi), Number(se), ms);
  if (!Number.isFinite(local)) return null;
  return { local, offset: m[8] ? parseOffset(m[8]) : null };
}

/** "+03:00" / "-0530" / "Z" → minutes east of UTC, or null. */
export function parseOffset(s: unknown): number | null {
  if (typeof s !== 'string') return null;
  const t = s.trim();
  if (/^z$/i.test(t)) return 0;
  const m = /^([+-])(\d{1,2}):?(\d{2})?$/.exec(t);
  if (!m) return null;
  const v = (m[1] === '-' ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3] ?? 0));
  return Math.abs(v) <= 14 * 60 ? v : null;
}

const fmtCache = new Map<string, Intl.DateTimeFormat>();

function formatter(timeZone: string): Intl.DateTimeFormat | null {
  let f = fmtCache.get(timeZone);
  if (f) return f;
  try {
    f = new Intl.DateTimeFormat('en-US', {
      timeZone,
      hourCycle: 'h23',
      year: 'numeric',
      month: 'numeric',
      day: 'numeric',
      hour: 'numeric',
      minute: 'numeric',
      second: 'numeric',
    });
  } catch {
    return null;
  }
  fmtCache.set(timeZone, f);
  return f;
}

/** true when `timeZone` is an IANA zone this runtime knows. */
export function isValidTimeZone(timeZone: unknown): timeZone is string {
  return typeof timeZone === 'string' && !!timeZone && formatter(timeZone) !== null;
}

/** Zone of the running process / device (falls back to UTC). */
export function systemTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  } catch {
    return 'UTC';
  }
}

/** Offset (minutes east of UTC) of `timeZone` at a given instant — DST correct. */
export function tzOffsetAt(instant: number, timeZone: string): number {
  if (!Number.isFinite(instant)) return 0;
  const f = formatter(timeZone);
  if (!f || timeZone === 'UTC') return 0;
  const parts = f.formatToParts(new Date(instant));
  const get = (t: string) => Number(parts.find((p) => p.type === t)?.value ?? 0);
  const asUtc = Date.UTC(get('year'), get('month') - 1, get('day'), get('hour') % 24, get('minute'), get('second'));
  const secs = Math.floor(instant / 1000) * 1000;
  return Math.round((asUtc - secs) / MIN);
}

/** Instant → wall clock in `timeZone`. */
export function instantToLocal(instant: number, timeZone: string): number {
  return instant + tzOffsetAt(instant, timeZone) * MIN;
}

/**
 * Wall clock in `timeZone` → instant (DST correct; ambiguous/non-existent times pick a neighbour).
 */
export function localToInstant(local: number, timeZone: string): number {
  const HALF_DAY = 12 * 3600_000;
  // offsets just before and after a possible transition around this wall clock
  const tA = local - tzOffsetAt(local - HALF_DAY, timeZone) * MIN;
  const tB = local - tzOffsetAt(local + HALF_DAY, timeZone) * MIN;
  if (tA === tB) return tA;
  const okA = instantToLocal(tA, timeZone) === local;
  const okB = instantToLocal(tB, timeZone) === local;
  if (okA && okB) return Math.min(tA, tB); // repeated hour (fall back) → earlier
  if (okB) return tB;
  return tA; // okA, or a skipped hour (spring forward) → shift forward like Temporal "compatible"
}

/**
 * Offset implied by the camera's local time and the GPS (UTC) time of the same shot,
 * rounded to 15 minutes; null when implausible (> ±14h).
 */
export function offsetFromGps(local: number, gpsUtc: number): number | null {
  if (!Number.isFinite(local) || !Number.isFinite(gpsUtc)) return null;
  const diff = Math.round((local - gpsUtc) / (15 * MIN)) * 15;
  return Math.abs(diff) <= 14 * 60 ? diff : null;
}

/** EXIF GPSDateStamp ("2024:07:01") + GPSTimeStamp ([h, m, s] or "10:00:00") → UTC ms. */
export function parseGpsDateTime(date: unknown, time: unknown): number | null {
  if (typeof date !== 'string') return null;
  const d = /^(\d{4})[:\-](\d{2})[:\-](\d{2})/.exec(date.trim());
  if (!d) return null;
  let h = 0;
  let m = 0;
  let s = 0;
  if (Array.isArray(time)) [h = 0, m = 0, s = 0] = time.map(Number);
  else if (typeof time === 'string') {
    const t = /^(\d{1,2}):(\d{1,2}):(\d{1,2}(?:\.\d+)?)/.exec(time.trim());
    if (!t) return null;
    [h, m, s] = [Number(t[1]), Number(t[2]), Number(t[3])];
  } else return null;
  if (![h, m, s].every(Number.isFinite)) return null;
  const v = Date.UTC(Number(d[1]), Number(d[2]) - 1, Number(d[3]), 0, 0, 0) + Math.round((h * 3600 + m * 60 + s) * 1000);
  return Number.isFinite(v) && Number(d[1]) > 1970 ? v : null;
}

export interface ResolvedTime {
  /** true instant */
  takenAt: number;
  /** wall clock as UTC */
  localAt: number;
  /** minutes east of UTC when actually known, else null */
  tzOffset: number | null;
}

/** Wall clock known (EXIF); offset optional → fall back to the home zone for the instant. */
export function resolveFromLocal(local: number, offset: number | null, homeTimeZone: string): ResolvedTime {
  if (offset !== null) return { takenAt: local - offset * MIN, localAt: local, tzOffset: offset };
  return { takenAt: localToInstant(local, homeTimeZone), localAt: local, tzOffset: null };
}

/** Only an instant is known (video creation_time, mtime) → wall clock in the home zone. */
export function resolveFromInstant(instant: number, homeTimeZone: string): ResolvedTime {
  return { takenAt: instant, localAt: instantToLocal(instant, homeTimeZone), tzOffset: null };
}

/** 'YYYY-MM-DD' of a localAt value. */
export function localDayKey(localAt: number): string {
  return new Date(localAt).toISOString().slice(0, 10);
}

/** Start of the (UTC-encoded) local day. */
export function localDayStart(localAt: number): number {
  return Math.floor(localAt / DAY) * DAY;
}

export const DAY_MS = DAY;
