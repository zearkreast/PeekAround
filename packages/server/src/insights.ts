import {
  COLOR_BUCKETS,
  colorIndexOfThumbhash,
  estimateInstant,
  GOLDEN_MAX,
  GOLDEN_MIN,
  isValidGps,
  LONG_VIDEO_S,
  MEDIA_FLAG,
  NIGHT_MAX,
  SHORT_VIDEO_S,
  sunAltitude,
  type BrowseQuery,
} from '@peekaround/shared';
import type { Db } from './db.js';
import { TIME_SRC } from './media/metadata.js';

/**
 * Explore insight columns (migration 004) + their browse filters.
 * color / sun are derived from other columns: computed lazily in SQL (fillInsightColumns) and reset
 * to NULL by triggers when their inputs change, so no writer (scanner, phones, remote folders) has
 * to know about them.
 */

/** media.sun value for rows where the sun position can't be known (no GPS / file date only) */
export const SUN_NA = 999;

const registered = new WeakSet<Db>();

export function registerInsightFunctions(db: Db) {
  if (registered.has(db)) return;
  registered.add(db);
  const det = { deterministic: true, useBigIntArguments: false };
  db.raw.function('peek_color', det, (hash: unknown) => colorIndexOfThumbhash(typeof hash === 'string' ? hash : null));
  db.raw.function('peek_sun', det, (takenAt: unknown, takenLocal: unknown, timeSrc: unknown, lat: unknown, lon: unknown) => sunOf(takenAt, takenLocal, timeSrc, lat, lon));
}

/** sun altitude at capture (rounded to 0.1°), SUN_NA when unknown */
export function sunOf(takenAt: unknown, takenLocal: unknown, timeSrc: unknown, lat: unknown, lon: unknown): number {
  if (typeof takenAt !== 'number' || !isValidGps(lat, lon)) return SUN_NA;
  const src = Number(timeSrc);
  if (src === TIME_SRC.mtime) return SUN_NA;
  // wall clock without offset: the home-zone instant may be hours off abroad → longitude estimate
  const instant = src === TIME_SRC.local && typeof takenLocal === 'number' ? estimateInstant(takenLocal, takenAt, lon as number) : takenAt;
  const a = sunAltitude(instant, lat as number, lon as number);
  return Number.isFinite(a) ? Math.round(a * 10) / 10 : SUN_NA;
}

let lastFill = new WeakMap<Db, number>();

/**
 * Compute missing color / sun values (cheap: header math / a few trig calls per row), at most
 * `limit` rows per column and call. Returns false when rows are left (callers continue in the
 * background). `minGapMs` keeps bursts of browse queries from re-checking.
 */
export function fillInsightColumns(db: Db, minGapMs = 0, limit = 1_000_000): boolean {
  const now = Date.now();
  if (minGapMs && now - (lastFill.get(db) ?? 0) < minGapMs) return true;
  lastFill.set(db, now);
  registerInsightFunctions(db);
  const a = Number(
    db.run(
      "UPDATE media SET color = peek_color(thumbhash) WHERE id IN (SELECT id FROM media WHERE color IS NULL AND thumbhash IS NOT NULL LIMIT ?)",
      limit,
    ).changes,
  );
  const b = Number(db.run('UPDATE media SET sun = peek_sun(taken_at, taken_local, time_src, lat, lon) WHERE id IN (SELECT id FROM media WHERE sun IS NULL LIMIT ?)', limit).changes);
  return a < limit && b < limit;
}

/** tests */
export function _resetInsightFill() {
  lastFill = new WeakMap();
}

const HOUR_SQL = "CAST(strftime('%H', taken_local / 1000, 'unixepoch') AS INTEGER)";

export interface SqlScope {
  where: string[];
  params: unknown[];
}

/** true when the query uses an insight filter (browse then flattens sub folders) */
export function hasInsightFilters(q: BrowseQuery): boolean {
  return !!(q.gps || q.color || q.orient || q.daytime || q.vlen || q.dup || q.city);
}

/** WHERE parts of the insight filters (gps, color, orient, daytime, vlen, dup, city) */
export function insightClauses(db: Db, q: BrowseQuery, s: SqlScope) {
  if (q.color || q.daytime) fillInsightColumns(db, 2000);
  if (q.gps === 'none') s.where.push('lat IS NULL');
  else if (q.gps === 'any') s.where.push('lat IS NOT NULL');
  if (q.color) {
    const i = COLOR_BUCKETS.indexOf(q.color);
    s.where.push(i < 0 ? '0' : 'color = ?');
    if (i >= 0) s.params.push(i);
  }
  if (q.orient) {
    const pano = `(flags & ${MEDIA_FLAG.panorama}) != 0`;
    if (q.orient === 'panorama') s.where.push(pano);
    else {
      s.where.push(`NOT ${pano}`, 'width > 0 AND height > 0');
      if (q.orient === 'square') s.where.push('abs(width - height) <= 0.05 * max(width, height)');
      else if (q.orient === 'portrait') s.where.push('height > width AND abs(width - height) > 0.05 * max(width, height)');
      else s.where.push('width > height AND abs(width - height) > 0.05 * max(width, height)');
    }
  }
  if (q.daytime === 'golden') s.where.push(`sun >= ${GOLDEN_MIN} AND sun <= ${GOLDEN_MAX}`);
  else if (q.daytime === 'night') {
    s.where.push(`(sun < ${NIGHT_MAX} OR (sun = ${SUN_NA} AND time_src != ${TIME_SRC.mtime} AND (${HOUR_SQL} >= 22 OR ${HOUR_SQL} < 5)))`);
  }
  if (q.vlen === 'short') s.where.push(`kind = 'video' AND duration > 0 AND duration < ${SHORT_VIDEO_S}`);
  else if (q.vlen === 'long') s.where.push(`kind = 'video' AND duration > ${LONG_VIDEO_S}`);
  if (q.dup) s.where.push('chash IN (SELECT chash FROM media WHERE chash IS NOT NULL GROUP BY chash HAVING count(*) > 1)');
  if (q.city) {
    s.where.push('city = ?');
    s.params.push(q.city);
  }
}
