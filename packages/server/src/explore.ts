import {
  cameraKey,
  computeInsights,
  type InsightRow,
  findTrips,
  instantToLocal,
  localDayKey,
  MEDIA_FLAG,
  onThisDay,
  type BrowseQuery,
  type CameraGroup,
  type ExploreResult,
  type MediaFlagName,
  type PlaceGroup,
  type TypeGroup,
  type YearGroup,
} from '@peekaround/shared';
import type { Ctx } from './context.js';
import { DuplicateService } from './duplicates.js';
import { placeKey } from './geo.js';
import { fillInsightColumns, SUN_NA } from './insights.js';
import { TIME_SRC } from './media/metadata.js';
import { resolveScope, whereSql } from './routes/browse.js';

/** one image/video row: the legacy short names used below + the InsightRow fields (same object) */
interface Row extends InsightRow {
  k: string;
  t: number;
  a1: string | null;
  make: string | null;
  model: string | null;
}

/** min spacing of {type:'explore'} events */
const EXPLORE_EVENT_MS = 10_000;
const MAX_TRIPS = 24;
/** derived insight columns computed per step (≈ 150 ms for 20k rows) */
const FILL_CHUNK = 20_000;

/**
 * Explore (automatic grouping) results, cached in memory per query and invalidated whenever media
 * are added/removed or their metadata changes; clients are told with a throttled {type:'explore'}.
 */
export class ExploreService {
  private cache = new Map<string, ExploreResult>();
  private timer: NodeJS.Timeout | null = null;
  private lastEvent = 0;
  private dirty = false;
  /** exact duplicates (lazy content hashes of same-size files) */
  readonly dups: DuplicateService;

  constructor(private ctx: Ctx) {
    this.dups = new DuplicateService(ctx);
    this.dups.onChanged = () => this.invalidate(true);
    ctx.events.on((e) => {
      if (e.type === 'media' && e.reason !== 'thumb') this.invalidate(e.reason === 'add' || e.reason === 'remove');
      else if (e.type === 'folders') this.invalidate(false);
    });
    ctx.scanner.onGroupsChanged = () => this.invalidate(true);
  }

  /** Drop cached results; `notify` also schedules a (throttled) explore event. */
  invalidate(notify = true) {
    this.cache.clear();
    if (!notify) return;
    this.dirty = true;
    if (this.timer) return;
    const delay = Math.max(1000, this.lastEvent + EXPLORE_EVENT_MS - Date.now());
    this.timer = setTimeout(() => {
      this.timer = null;
      if (!this.dirty) return;
      this.dirty = false;
      this.lastEvent = Date.now();
      this.ctx.events.emit({ type: 'explore' });
    }, delay);
    this.timer.unref?.();
  }

  private filling: NodeJS.Timeout | null = null;

  /** keep computing colour / sun columns in chunks, then refresh Explore */
  private fillLater() {
    if (this.filling) return;
    const step = () => {
      this.filling = null;
      let done = true;
      try {
        done = fillInsightColumns(this.ctx.db, 0, FILL_CHUNK);
      } catch {
        return; // closed
      }
      if (done) this.invalidate(true);
      else this.filling = setTimeout(step, 50);
    };
    this.filling = setTimeout(step, 50);
    this.filling.unref?.();
  }

  close() {
    if (this.filling) clearTimeout(this.filling);
    this.filling = null;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    void this.dups.stop();
  }

  get(q: BrowseQuery, now = Date.now()): ExploreResult {
    const nowLocal = instantToLocal(now, this.ctx.scanner.homeTimeZone);
    const key = `${localDayKey(nowLocal)}|${JSON.stringify(q)}`;
    let r = this.cache.get(key);
    if (!r) {
      r = this.compute(q, nowLocal);
      if (this.cache.size > 32) this.cache.clear();
      this.cache.set(key, r);
    }
    return { ...r, indexing: this.ctx.scanner.indexing() };
  }

  compute(q: BrowseQuery, nowLocal: number): ExploreResult {
    const { s } = resolveScope(this.ctx, { ...q, kind: q.kind && q.kind !== 'all' ? q.kind : ('image-video' as BrowseQuery['kind']), recursive: true });
    // derived colour / sun columns: a first chunk now, the rest in the background (first run after an upgrade)
    if (!fillInsightColumns(this.ctx.db, 0, FILL_CHUNK)) this.fillLater();
    // rows as arrays (≈ 2× faster than objects for 100k rows), turned into one object per item
    const arr = this.ctx.db.allArrays(
      `SELECT id, kind, taken_local, lat, lon, cc, admin1, city, make, model, flags, size, width, height, duration, color, sun, time_src, added_at, mtime FROM media ${whereSql(s)} ORDER BY taken_local DESC, id DESC`,
      ...s.params,
    ) as [number, string, number, number | null, number | null, string | null, string | null, string | null, string | null, string | null, number, number, number | null, number | null, number | null, number | null, number | null, number, number | null, number][];
    const rows: Row[] = new Array(arr.length);
    for (let i = 0; i < arr.length; i++) {
      const [id, k, t, lat, lon, cc, a1, city, make, model, flags, size, width, height, duration, color, sun, src, added, mtime] = arr[i];
      rows[i] = {
        id: String(id),
        k,
        t,
        kind: k,
        localAt: t,
        lat,
        lon,
        cc,
        a1,
        city,
        make,
        model,
        flags,
        size,
        width,
        height,
        duration,
        color,
        sun: sun === null || sun === SUN_NA ? null : sun,
        clockKnown: src !== TIME_SRC.mtime,
        addedAt: added,
        mtime,
        placeKey: cc ? placeKey(cc, a1) : null,
      };
    }
    const geo = this.ctx.geo;

    // ── memories ──
    const otd = onThisDay(rows, nowLocal);
    const groupName = (cc: string, a1: string | null, city: string | null) => geo.admin1Name(cc, a1) ?? city ?? cc;
    const trips = findTrips(
      rows.map((r) => ({
        id: String(r.id),
        localAt: r.t,
        lat: r.lat,
        lon: r.lon,
        placeKey: r.cc ? placeKey(r.cc, r.a1) : null,
        placeName: r.cc ? groupName(r.cc, r.a1, r.city) : null,
        cc: r.cc,
      })),
    ).slice(0, MAX_TRIPS);

    // ── places (country + admin1) ──
    const places = new Map<string, { g: PlaceGroup; lat: number; lon: number; n: number; cities: Map<string, number> }>();
    for (const r of rows) {
      if (!r.cc || r.lat == null || r.lon == null) continue;
      const k = placeKey(r.cc, r.a1);
      let p = places.get(k);
      if (!p) {
        p = {
          g: { key: k, cc: r.cc, name: '', city: null, count: 0, coverId: String(r.id), lat: 0, lon: 0 },
          lat: 0,
          lon: 0,
          n: 0,
          cities: new Map(),
        };
        places.set(k, p);
      }
      p.g.count++;
      p.lat += r.lat;
      p.lon += r.lon;
      p.n++;
      if (r.city) p.cities.set(r.city, (p.cities.get(r.city) ?? 0) + 1);
    }
    const placeList: PlaceGroup[] = [...places.values()].map((p) => {
      let city: string | null = null;
      let best = 0;
      for (const [c, n] of p.cities) if (n > best) [city, best] = [c, n];
      const a1 = p.g.key.slice(p.g.cc.length + 1);
      return {
        ...p.g,
        name: geo.admin1Name(p.g.cc, a1 === '00' ? null : a1) ?? city ?? p.g.cc,
        city,
        lat: Math.round((p.lat / p.n) * 1e5) / 1e5,
        lon: Math.round((p.lon / p.n) * 1e5) / 1e5,
      };
    });
    placeList.sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));

    // ── types ──
    const types: TypeGroup[] = [];
    const typeOf: [MediaFlagName, (r: Row) => boolean][] = [
      ['video', (r) => r.k === 'video'],
      ['selfie', (r) => (r.flags & MEDIA_FLAG.selfie) !== 0],
      ['screenshot', (r) => (r.flags & MEDIA_FLAG.screenshot) !== 0],
      ['panorama', (r) => (r.flags & MEDIA_FLAG.panorama) !== 0],
      ['animated', (r) => (r.flags & MEDIA_FLAG.animated) !== 0],
    ];
    for (const [flag, test] of typeOf) {
      let count = 0;
      let coverId = '';
      for (const r of rows) {
        if (!test(r)) continue;
        if (!count) coverId = String(r.id);
        count++;
      }
      if (count) types.push({ flag, count, coverId });
    }

    // ── cameras ──
    const cams = new Map<string, CameraGroup>();
    for (const r of rows) {
      if (!r.model) continue;
      const k = cameraKey(r.make, r.model);
      const c = cams.get(k);
      if (c) c.count++;
      else cams.set(k, { key: k, make: r.make, model: r.model, count: 1, coverId: String(r.id) });
    }
    const cameras = [...cams.values()].sort((a, b) => b.count - a.count || a.model.localeCompare(b.model));

    // ── years ──
    const years = new Map<number, YearGroup>();
    for (const r of rows) {
      const y = new Date(r.t).getUTCFullYear();
      const g = years.get(y);
      if (g) g.count++;
      else years.set(y, { year: y, count: 1, coverId: String(r.id) });
    }

    // ── insights (stats, year in review, countries, colours, time of day, orientation, …) ──
    const insights = computeInsights(rows);
    const duplicates = this.dups.summary(whereSql(s), s.params);
    if (duplicates.pending) this.dups.kick();

    return {
      ...insights,
      duplicates: duplicates.groups || duplicates.pending ? duplicates : undefined,
      memories: [...otd, ...trips],
      places: placeList,
      types,
      cameras,
      years: [...years.values()].sort((a, b) => b.year - a.year),
      indexing: { done: 0, total: 0 },
    };
  }
}
