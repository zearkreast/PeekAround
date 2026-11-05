/**
 * Trip detection — pure and deterministic.
 *
 * Home is computed PER YEAR: the place group (PlaceInfo.key) with the most distinct photo days.
 * Other groups that were lived in for a while (≥ half the top's days, spread over ≥ 60 days)
 * also count as home, so moving city mid-year does not turn the old/new city into a "trip".
 * A trip = consecutive days (gap ≤ 1 empty day, no home day in between) whose GPS items are mostly
 * ≥ 50 km from every home and outside the home groups; kept when it spans ≥ 2 days or has ≥ 10 items.
 */
import type { Memory } from './index';
import { distanceKm, isValidGps } from './geo';
import { spreadPick } from './memories';
import { DAY_MS, localDayKey, localDayStart } from './time';

export interface TripItem {
  id: string;
  localAt: number;
  lat?: number | null;
  lon?: number | null;
  /** PlaceInfo.key (`${cc}-${admin1}`) */
  placeKey?: string | null;
  /** display name of the place group (admin1 name or city) */
  placeName?: string | null;
  cc?: string | null;
}

export interface TripOptions {
  minKm?: number;
  minDays?: number;
  minItems?: number;
  /** empty days allowed between two trip days */
  maxGapDays?: number;
  covers?: number;
}

export interface Home {
  key: string;
  lat: number;
  lon: number;
  days: number;
}

const median = (v: number[]) => {
  const s = [...v].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};

const groupKey = (it: TripItem) => it.placeKey || `@${Math.round((it.lat as number) * 2) / 2},${Math.round((it.lon as number) * 2) / 2}`;

/** Home place(s) per calendar year (of localAt). */
export function homesByYear(items: TripItem[]): Map<number, Home[]> {
  const years = new Map<number, Map<string, { days: Set<number>; lats: number[]; lons: number[]; first: number; last: number }>>();
  for (const it of items) {
    if (!isValidGps(it.lat, it.lon)) continue;
    const y = new Date(it.localAt).getUTCFullYear();
    let g = years.get(y);
    if (!g) years.set(y, (g = new Map()));
    const k = groupKey(it);
    let e = g.get(k);
    const day = localDayStart(it.localAt);
    if (!e) g.set(k, (e = { days: new Set(), lats: [], lons: [], first: day, last: day }));
    e.days.add(day);
    e.lats.push(it.lat as number);
    e.lons.push(it.lon as number);
    e.first = Math.min(e.first, day);
    e.last = Math.max(e.last, day);
  }
  const out = new Map<number, Home[]>();
  for (const [y, g] of years) {
    const ranked = [...g.entries()].sort((a, b) => b[1].days.size - a[1].days.size || (a[0] < b[0] ? -1 : 1));
    const top = ranked[0][1].days.size;
    const homes: Home[] = [];
    ranked.forEach(([key, e], i) => {
      const lived = e.days.size >= Math.max(3, top / 2) && e.last - e.first >= 60 * DAY_MS;
      if (i === 0 || lived) homes.push({ key, lat: median(e.lats), lon: median(e.lons), days: e.days.size });
    });
    out.set(y, homes);
  }
  return out;
}

export function findTrips(items: TripItem[], opts: TripOptions = {}): Memory[] {
  const minKm = opts.minKm ?? 50;
  const minDays = opts.minDays ?? 2;
  const minItems = opts.minItems ?? 10;
  const maxGap = opts.maxGapDays ?? 1;
  const covers = opts.covers ?? 6;
  const homes = homesByYear(items);

  // years without enough GPS data borrow the nearest year's homes
  const known = [...homes.entries()].filter(([, h]) => h[0].days >= 3).map(([y]) => y).sort((a, b) => a - b);
  const homesFor = (y: number): Home[] => {
    const h = homes.get(y);
    if (h && h[0].days >= 3) return h;
    if (!known.length) return h ?? [];
    const near = known.reduce((best, k) => (Math.abs(k - y) < Math.abs(best - y) ? k : best), known[0]);
    return homes.get(near)!;
  };

  const sorted = [...items].sort((a, b) => a.localAt - b.localAt || (a.id < b.id ? -1 : 1));
  interface Day {
    start: number;
    gps: number;
    away: number;
  }
  const days: Day[] = [];
  for (const it of sorted) {
    const start = localDayStart(it.localAt);
    let d = days[days.length - 1];
    if (!d || d.start !== start) days.push((d = { start, gps: 0, away: 0 }));
    if (!isValidGps(it.lat, it.lon)) continue;
    d.gps++;
    const hs = homesFor(new Date(start).getUTCFullYear());
    const k = groupKey(it);
    const away = hs.length > 0 && hs.every((h) => h.key !== k && distanceKm(h.lat, h.lon, it.lat as number, it.lon as number) >= minKm);
    if (away) d.away++;
  }

  const trips: { first: number; last: number; awayDays: number }[] = [];
  let cur: { first: number; last: number; awayDays: number } | null = null;
  for (const d of days) {
    if (d.gps === 0) continue; // neutral
    const isAway = d.away * 2 >= d.gps;
    if (!isAway) {
      if (cur) trips.push(cur);
      cur = null;
      continue;
    }
    if (cur && d.start - cur.last <= (maxGap + 1) * DAY_MS) {
      cur.last = d.start;
      cur.awayDays++;
    } else {
      if (cur) trips.push(cur);
      cur = { first: d.start, last: d.start, awayDays: 1 };
    }
  }
  if (cur) trips.push(cur);

  const out: Memory[] = [];
  for (const t of trips) {
    const from = t.first;
    const to = t.last + DAY_MS;
    const inside = sorted.filter((it) => it.localAt >= from && it.localAt < to);
    if (t.awayDays < minDays && inside.length < minItems) continue;
    // dominant place group among the away items
    const tally = new Map<string, { n: number; name: string | null; cc: string | null }>();
    for (const it of inside) {
      if (!isValidGps(it.lat, it.lon) || !it.placeName) continue;
      const k = groupKey(it);
      const e = tally.get(k) ?? { n: 0, name: it.placeName, cc: it.cc ?? null };
      e.n++;
      tally.set(k, e);
    }
    const top = [...tally.values()].sort((a, b) => b.n - a.n)[0];
    out.push({
      id: `trip-${localDayKey(from)}`,
      kind: 'trip',
      place: top?.name ?? undefined,
      cc: top?.cc ?? undefined,
      from,
      to,
      count: inside.length,
      coverIds: spreadPick(inside, covers),
    });
  }
  return out.sort((a, b) => b.from - a.from);
}
