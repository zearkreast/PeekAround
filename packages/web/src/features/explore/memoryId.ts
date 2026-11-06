import { DAY_MS, type Memory } from '@peekaround/shared';

/**
 * A memory id that is no longer in the Explore list (the day changed, a deep link…):
 * on-this-day ids encode their anniversary date (window ±3 days), year reviews their year; trips can't be rebuilt.
 */
export function memoryFromId(id: string, now = Date.now()): Memory | null {
  // year in review: the calendar year
  const y = /^year-(\d{4})$/.exec(id);
  if (y) {
    const year = Number(y[1]);
    return { id, kind: 'year', from: Date.UTC(year, 0, 1), to: Date.UTC(year + 1, 0, 1), count: 0, coverIds: [] };
  }
  const m = /^otd-(\d{4})-(\d{2})-(\d{2})$/.exec(id);
  if (!m) return null;
  const day = Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  if (!Number.isFinite(day)) return null;
  const yearsAgo = Math.max(1, new Date(now).getUTCFullYear() - Number(m[1]));
  return { id, kind: 'onthisday', yearsAgo, from: day - 3 * DAY_MS, to: day + 4 * DAY_MS, count: 0, coverIds: [] };
}
