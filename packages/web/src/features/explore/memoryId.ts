import { DAY_MS, type Memory } from '@peekaround/shared';

/**
 * A memory id that is no longer in the Explore list (the day changed, a deep link…):
 * on-this-day ids encode their anniversary date (window ±3 days); trips can't be rebuilt.
 */
export function memoryFromId(id: string, now = Date.now()): Memory | null {
  const m = /^otd-(\d{4})-(\d{2})-(\d{2})$/.exec(id);
  if (!m) return null;
  const day = Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  if (!Number.isFinite(day)) return null;
  const yearsAgo = Math.max(1, new Date(now).getUTCFullYear() - Number(m[1]));
  return { id, kind: 'onthisday', yearsAgo, from: day - 3 * DAY_MS, to: day + 4 * DAY_MS, count: 0, coverIds: [] };
}
