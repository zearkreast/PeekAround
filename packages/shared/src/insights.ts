/**
 * Explore "insights" (stats header, year in review, countries → regions → cities, time of day,
 * colours, orientation, video length, largest / recently added, calendar) — pure and shared by the
 * server (rows from SQL) and the Android local backend (items + native index records).
 */
import type { Memory } from './index';
import { COLOR_BUCKETS, type ColorBucket } from './color';
import { spreadPick } from './memories';
import { GOLDEN_MAX, GOLDEN_MIN, isNightHour, NIGHT_MAX } from './sun';
import { DAY_MS } from './time';

export type Orientation = 'portrait' | 'landscape' | 'square' | 'panorama';
export const ORIENTATIONS: readonly Orientation[] = ['portrait', 'landscape', 'square', 'panorama'];
export type VideoLength = 'short' | 'long';
export const VIDEO_LENGTHS: readonly VideoLength[] = ['short', 'long'];
export type DayTime = 'night' | 'golden';
export const DAY_TIMES: readonly DayTime[] = ['night', 'golden'];

/** short clips: < 15 s; long videos: > 5 min */
export const SHORT_VIDEO_S = 15;
export const LONG_VIDEO_S = 300;
/** a year gets a "year in review" memory from this many items on */
export const YEAR_REVIEW_MIN = 12;

const PANORAMA_FLAG = 4; // MEDIA_FLAG.panorama (kept local to avoid an import cycle)

export function isOrientation(s: unknown): s is Orientation {
  return typeof s === 'string' && (ORIENTATIONS as readonly string[]).includes(s);
}
export function isVideoLength(s: unknown): s is VideoLength {
  return s === 'short' || s === 'long';
}
export function isDayTime(s: unknown): s is DayTime {
  return s === 'night' || s === 'golden';
}

/** orientation bucket (same rules as the server's SQL filter) */
export function orientationOf(w: number | null | undefined, h: number | null | undefined, flags = 0): Orientation | null {
  if ((flags & PANORAMA_FLAG) !== 0) return 'panorama';
  if (!w || !h || w <= 0 || h <= 0) return null;
  if (Math.abs(w - h) <= 0.05 * Math.max(w, h)) return 'square';
  return h > w ? 'portrait' : 'landscape';
}

export function videoLengthOf(duration: number | null | undefined): VideoLength | null {
  if (duration == null || !Number.isFinite(duration) || duration <= 0) return null;
  if (duration < SHORT_VIDEO_S) return 'short';
  if (duration > LONG_VIDEO_S) return 'long';
  return null;
}

/**
 * night / golden hour of one item: from the sun altitude when known (GPS + time), otherwise
 * night by the wall clock (never for file-date-only items, whose clock is not the capture time).
 */
export function dayTimeOf(sun: number | null | undefined, localAt: number, clockKnown: boolean): DayTime | null {
  if (sun != null && Number.isFinite(sun)) {
    if (sun < NIGHT_MAX) return 'night';
    if (sun >= GOLDEN_MIN && sun <= GOLDEN_MAX) return 'golden';
    return null;
  }
  if (!clockKnown) return null;
  return isNightHour(new Date(localAt).getUTCHours()) ? 'night' : null;
}

// ───────────────────────── result types ─────────────────────────

export interface ExploreStats {
  photos: number;
  videos: number;
  countries: number;
  cities: number;
  /** year span of the capture dates (null when empty) */
  firstYear: number | null;
  lastYear: number | null;
  /** total size of the images + videos (local + cloud) */
  bytes: number;
  /** part of `bytes` in cloud folders (MEGA links streamed, not on this device); missing from older servers */
  cloudBytes?: number;
  withLocation: number;
}

export interface FacetGroup<T extends string = string> {
  value: T;
  count: number;
  coverId: string;
  /** a few more covers (newest first) for collage cards */
  coverIds?: string[];
}

export interface CountryGroup {
  cc: string;
  count: number;
  coverId: string;
  /** distinct admin1 regions / cities */
  regions: number;
  cities: number;
  lat: number;
  lon: number;
}

export interface CityGroup {
  /** `${placeKey}|${city}` */
  key: string;
  cc: string;
  /** PlaceGroup key (`${cc}-${admin1}`) the city belongs to */
  placeKey: string;
  name: string;
  count: number;
  coverId: string;
  lat: number;
  lon: number;
}

/** one item of a ranked preview row (largest: bytes, recent: added time) */
export interface RankedItem {
  id: string;
  value: number;
}

export interface DuplicateSummary {
  /** groups of identical files and the files in them */
  groups: number;
  files: number;
  /** bytes that could be freed by keeping one copy per group */
  wasted: number;
  /** same-size candidates still waiting to be hashed */
  pending: number;
  coverIds: string[];
}

export interface ExploreExtras {
  stats?: ExploreStats;
  /** "year in review" memories (kind 'year'), newest year first */
  yearReviews?: Memory[];
  countries?: CountryGroup[];
  cities?: CityGroup[];
  dayTimes?: FacetGroup<DayTime>[];
  colors?: FacetGroup<ColorBucket>[];
  orientations?: FacetGroup<Orientation>[];
  videoLengths?: FacetGroup<VideoLength>[];
  largest?: RankedItem[];
  recent?: RankedItem[];
  duplicates?: DuplicateSummary;
  /** years that have items (calendar heatmap picker), newest first */
  calendarYears?: number[];
  /** image/video items without a location */
  noLocation?: number;
}

export interface CalendarYear {
  year: number;
  /** items per day of the year (index 0 = Jan 1; 365 or 366 entries) */
  days: number[];
  total: number;
  max: number;
}

export interface DuplicateGroup {
  hash: string;
  size: number;
  ids: string[];
}

export interface MetaLibraryStatus {
  id: string;
  name: string;
  total: number;
  withLocation: number;
  /** read, and no location (withLocation + withoutLocation + pending + unreadable = total) */
  withoutLocation: number;
  /** no location yet and still waiting for (re-)reading */
  pending: number;
  /** files whose metadata could not be read */
  unreadable: number;
  /** files that look like they came through messaging apps (WhatsApp, Telegram …) — location stripped */
  messaging: number;
  /** location from Google Takeout JSON sidecars */
  sidecar?: number;
}

export interface MetaStatus {
  libraries: MetaLibraryStatus[];
  enriching: boolean;
  indexing: { done: number; total: number };
}

// ───────────────────────── aggregation ─────────────────────────

export interface InsightRow {
  id: string;
  kind: string;
  localAt: number;
  size: number;
  width: number | null;
  height: number | null;
  duration: number | null;
  lat: number | null;
  lon: number | null;
  cc: string | null;
  /** PlaceGroup key of the row (`${cc}-${admin1 || '00'}`) */
  placeKey: string | null;
  city: string | null;
  flags: number;
  /** index into COLOR_BUCKETS, -1/null = unknown */
  color?: number | null;
  sun?: number | null;
  /** capture clock known (not the file date) */
  clockKnown?: boolean;
  /** added to the library (ms), falls back to mtime */
  addedAt?: number | null;
  mtime: number;
  /** lives in a cloud (remote / MEGA) folder, not on this device */
  cloud?: boolean;
}

export interface InsightOptions {
  /** skip sections the caller cannot compute */
  colors?: boolean;
  dayTimes?: boolean;
  covers?: number;
  ranked?: number;
  maxCities?: number;
}

function topK(list: RankedItem[], k: number, it: RankedItem) {
  if (list.length < k) list.push(it);
  else if (it.value <= list[k - 1].value) return;
  else list[k - 1] = it;
  // insertion: keep descending order (ties: the earlier = newer row stays first)
  let i = list.length - 1;
  while (i > 0 && list[i - 1].value < it.value) {
    list[i] = list[i - 1];
    i--;
  }
  list[i] = it;
}

/** rows: images + videos, newest (localAt) first */
export function computeInsights(rows: InsightRow[], opts: InsightOptions = {}): ExploreExtras {
  const covers = opts.covers ?? 4;
  const ranked = opts.ranked ?? 20;
  const stats: ExploreStats = { photos: 0, videos: 0, countries: 0, cities: 0, firstYear: null, lastYear: null, bytes: 0, cloudBytes: 0, withLocation: 0 };
  let cloudBytes = 0;
  const facet = <T extends string>() => new Map<T, FacetGroup<T>>();
  const add = <T extends string>(m: Map<T, FacetGroup<T>>, v: T, id: string) => {
    const g = m.get(v);
    if (!g) m.set(v, { value: v, count: 1, coverId: id, coverIds: [id] });
    else {
      g.count++;
      if (g.coverIds!.length < covers) g.coverIds!.push(id);
    }
  };
  const dayTimes = facet<DayTime>();
  const colors = facet<ColorBucket>();
  const orients = facet<Orientation>();
  const lengths = facet<VideoLength>();
  const countries = new Map<string, CountryGroup & { la: number; lo: number; regionSet: Set<string>; citySet: Set<string> }>();
  const cities = new Map<string, CityGroup & { la: number; lo: number }>();
  const years = new Map<number, InsightRow[]>();
  const largest: RankedItem[] = [];
  const recent: RankedItem[] = [];
  let noLocation = 0;

  for (const r of rows) {
    if (r.kind === 'video') stats.videos++;
    else stats.photos++;
    stats.bytes += r.size || 0;
    if (r.cloud) cloudBytes += r.size || 0;
    const y = new Date(r.localAt).getUTCFullYear();
    let yl = years.get(y);
    if (!yl) years.set(y, (yl = []));
    yl.push(r);

    const hasGps = r.lat != null && r.lon != null;
    if (hasGps) stats.withLocation++;
    else noLocation++;
    if (r.cc && hasGps) {
      let c = countries.get(r.cc);
      if (!c) countries.set(r.cc, (c = { cc: r.cc, count: 0, coverId: r.id, regions: 0, cities: 0, lat: 0, lon: 0, la: 0, lo: 0, regionSet: new Set(), citySet: new Set() }));
      c.count++;
      c.la += r.lat!;
      c.lo += r.lon!;
      if (r.placeKey) c.regionSet.add(r.placeKey);
      if (r.city) {
        c.citySet.add(r.city);
        const pk = r.placeKey ?? `${r.cc}-00`;
        const key = `${pk}|${r.city}`;
        let ci = cities.get(key);
        if (!ci) cities.set(key, (ci = { key, cc: r.cc, placeKey: pk, name: r.city, count: 0, coverId: r.id, lat: 0, lon: 0, la: 0, lo: 0 }));
        ci.count++;
        ci.la += r.lat!;
        ci.lo += r.lon!;
      }
    }

    if (opts.dayTimes !== false) {
      const dt = dayTimeOf(r.sun, r.localAt, r.clockKnown ?? true);
      if (dt) add(dayTimes, dt, r.id);
    }
    if (opts.colors !== false && r.color != null && r.color >= 0 && r.color < COLOR_BUCKETS.length) add(colors, COLOR_BUCKETS[r.color], r.id);
    const o = orientationOf(r.width, r.height, r.flags);
    if (o) add(orients, o, r.id);
    if (r.kind === 'video') {
      const l = videoLengthOf(r.duration);
      if (l) add(lengths, l, r.id);
    }
    if (r.size > 0) topK(largest, ranked, { id: r.id, value: r.size });
    topK(recent, ranked, { id: r.id, value: r.addedAt ?? r.mtime });
  }

  const yearList = [...years.keys()].sort((a, b) => b - a);
  stats.firstYear = yearList.length ? yearList[yearList.length - 1] : null;
  stats.lastYear = yearList.length ? yearList[0] : null;
  stats.countries = countries.size;
  stats.cloudBytes = cloudBytes;
  let cityCount = 0;
  for (const c of countries.values()) cityCount += c.citySet.size;
  stats.cities = cityCount;

  const ordered = <T extends string>(m: Map<T, FacetGroup<T>>, order: readonly T[]) => order.map((v) => m.get(v)).filter((g): g is FacetGroup<T> => !!g);

  return {
    stats,
    yearReviews: yearList.map((y) => yearReview(y, years.get(y)!)).filter((m): m is Memory => !!m),
    countries: [...countries.values()]
      .map(({ la, lo, regionSet, citySet, ...c }) => ({ ...c, regions: regionSet.size, cities: citySet.size, lat: round5(la / c.count), lon: round5(lo / c.count) }))
      .sort((a, b) => b.count - a.count || a.cc.localeCompare(b.cc)),
    cities: [...cities.values()]
      .map(({ la, lo, ...c }) => ({ ...c, lat: round5(la / c.count), lon: round5(lo / c.count) }))
      .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name))
      .slice(0, opts.maxCities ?? 800),
    dayTimes: opts.dayTimes === false ? undefined : ordered(dayTimes, DAY_TIMES),
    colors: opts.colors === false ? undefined : ordered(colors, COLOR_BUCKETS),
    orientations: ordered(orients, ORIENTATIONS),
    videoLengths: ordered(lengths, VIDEO_LENGTHS),
    largest,
    recent,
    calendarYears: yearList,
    noLocation,
  };
}

const round5 = (v: number) => Math.round(v * 1e5) / 1e5;

/**
 * "Year in review": the whole calendar year, covers spread over the months
 * (one per month that has items, newest month first, more from big months when < 6 months).
 * @param items of that year, newest first
 */
export function yearReview(year: number, items: { id: string; localAt: number }[], min = YEAR_REVIEW_MIN): Memory | null {
  if (items.length < min) return null;
  const months: { id: string; localAt: number }[][] = Array.from({ length: 12 }, () => []);
  for (const it of items) months[new Date(it.localAt).getUTCMonth()].push(it);
  const filled = months.map((m, i) => ({ m, i })).filter((x) => x.m.length > 0);
  const want = Math.min(12, Math.max(6, filled.length));
  const per = Math.max(1, Math.floor(want / filled.length));
  const covers: string[] = [];
  for (let k = filled.length - 1; k >= 0 && covers.length < want; k--) covers.push(...spreadPick(filled[k].m, per).slice(0, want - covers.length));
  return {
    id: `year-${year}`,
    kind: 'year',
    from: Date.UTC(year, 0, 1),
    to: Date.UTC(year + 1, 0, 1),
    count: items.length,
    coverIds: covers,
    months: months.map((m) => m.length),
  };
}

/** items per day of `year` from localAt values */
export function calendarOf(year: number, localAts: Iterable<number>): CalendarYear {
  const start = Date.UTC(year, 0, 1);
  const n = Math.round((Date.UTC(year + 1, 0, 1) - start) / DAY_MS);
  const days = new Array<number>(n).fill(0);
  let total = 0;
  for (const t of localAts) {
    const d = Math.floor((t - start) / DAY_MS);
    if (d < 0 || d >= n) continue;
    days[d]++;
    total++;
  }
  return { year, days, total, max: days.reduce((a, b) => Math.max(a, b), 0) };
}

/** "2024-03-05" of day index `d` of `year` */
export function calendarDayKey(year: number, d: number): string {
  return new Date(Date.UTC(year, 0, 1) + d * DAY_MS).toISOString().slice(0, 10);
}
