/**
 * "On this day" memories — pure and deterministic (`now` is a parameter).
 * All times are localAt values (wall clock encoded as UTC, see MediaItem.localAt).
 */
import type { Memory } from './index';
import { DAY_MS, localDayStart } from './time';

export interface MemoryItem {
  id: string;
  localAt: number;
}

export interface OnThisDayOptions {
  /** ± days around the month-day (default 3) */
  windowDays?: number;
  /** minimum items per year (default 3) */
  minItems?: number;
  /** cover ids per memory (default 6) */
  covers?: number;
  /** how many years back to look (default 30) */
  maxYears?: number;
}

/** Pick up to n ids spread evenly over the list. */
export function spreadPick<T extends { id: string }>(list: T[], n: number): string[] {
  if (list.length <= n) return list.map((x) => x.id);
  const out: string[] = [];
  for (let k = 0; k < n; k++) out.push(list[Math.floor(((k + 0.5) * list.length) / n)].id);
  return out;
}

function lowerBound(items: MemoryItem[], v: number): number {
  let lo = 0;
  let hi = items.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (items[mid].localAt < v) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * Items of previous years taken within ±windowDays of today's month-day.
 * @param items any order
 * @param nowLocal current wall-clock time in the home zone (as UTC ms)
 * Newest year first.
 */
export function onThisDay(items: MemoryItem[], nowLocal: number, opts: OnThisDayOptions = {}): Memory[] {
  const win = opts.windowDays ?? 3;
  const min = opts.minItems ?? 3;
  const covers = opts.covers ?? 6;
  const sorted = [...items].sort((a, b) => a.localAt - b.localAt || (a.id < b.id ? -1 : 1));
  if (!sorted.length) return [];
  const now = new Date(localDayStart(nowLocal));
  const out: Memory[] = [];
  for (let ago = 1; ago <= (opts.maxYears ?? 30); ago++) {
    const y = now.getUTCFullYear() - ago;
    // Feb 29 → Feb 28 in non-leap years
    let day = Date.UTC(y, now.getUTCMonth(), now.getUTCDate());
    if (new Date(day).getUTCMonth() !== now.getUTCMonth()) day = Date.UTC(y, now.getUTCMonth() + 1, 0);
    const from = day - win * DAY_MS;
    const to = day + (win + 1) * DAY_MS;
    // windows only move back in time; the window may reach into the next year (Dec 31 ↔ Jan 2)
    if (to <= sorted[0].localAt) break;
    const a = lowerBound(sorted, from);
    const b = lowerBound(sorted, to);
    if (b - a < min) continue;
    const slice = sorted.slice(a, b);
    out.push({ id: `otd-${new Date(day).toISOString().slice(0, 10)}`, kind: 'onthisday', yearsAgo: ago, from, to, count: slice.length, coverIds: spreadPick(slice, covers) });
  }
  return out;
}
