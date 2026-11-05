/**
 * Explore (automatic grouping) for Android local mode — same pure modules as the server
 * (classify, onThisDay, findTrips, normalizeCamera, offline reverse geocoding).
 *
 * MediaItems carry no GPS / camera, so metadata comes from the native PeekIndex (indexBridge.ts),
 * joined by absolute path (= decoded media id). Records are cached per id (+ mtime) so the
 * synchronous filters used by browse stay cheap for 20k items. Without the native index the groups
 * use dates, kinds and names only (screenshots, panoramas by name, animations, years, on this day).
 */
import {
  classify,
  createGeoIndex,
  findTrips,
  foldText,
  GEO_ASSET_PATH,
  isGzip,
  isValidGps,
  localDayKey,
  MEDIA_FLAG,
  normalizeCamera,
  onThisDay,
  parseSearch,
  type BrowseQuery,
  type CameraGroup,
  type ExploreResult,
  type GeoIndex,
  type GeoPoints,
  type MediaFlagName,
  type MediaItem,
  type PlaceGroup,
  type TypeGroup,
  type YearGroup,
} from '@peekaround/shared';
import { decodeId, ensureIndexed, INDEX_HINT, indexStatus, queryIndex, rootsOf, type IndexMeta, type IndexStatus } from './indexBridge';

const MAX_TRIPS = 24;

interface PlaceRef {
  key: string;
  name: string;
  city: string | null;
  cc: string;
}

interface Rec {
  mtime: number;
  /** null = not in the native index (yet) */
  meta: IndexMeta | null;
  flags: number;
  place: PlaceRef | null;
  /** place resolved with a loaded geo index (or no GPS at all) */
  placeDone: boolean;
  cam: { key: string; make: string | null; model: string } | null;
}

const recs = new Map<string, Rec>();
const paths = new Map<string, string>();
let version = 0;

// ───────────────────────── geo index (lazy, cached) ─────────────────────────

let geo: GeoIndex | null = null;
let geoP: Promise<GeoIndex | null> | null = null;

async function defaultGeoLoader(): Promise<Uint8Array> {
  const base = (import.meta as { env?: { BASE_URL?: string } }).env?.BASE_URL ?? '/';
  const r = await fetch(new URL(`${base}${GEO_ASSET_PATH}`, location.href));
  if (!r.ok) throw new Error(`geo asset: HTTP ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}
let geoLoader: () => Promise<Uint8Array> = defaultGeoLoader;

async function gunzip(b: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([b as Uint8Array<ArrayBuffer>]).stream().pipeThrough(new DecompressionStream('gzip'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** the offline reverse-geocoding index (GeoNames), loaded once from /geo/cities.bin */
export function loadGeo(): Promise<GeoIndex | null> {
  if (!geoP) {
    geoP = (async () => {
      try {
        let b = await geoLoader();
        if (isGzip(b)) b = await gunzip(b);
        geo = createGeoIndex(b);
        return geo;
      } catch (e) {
        console.warn('[explore] geo index unavailable', e);
        geoP = null; // retry next time
        return null;
      }
    })();
  }
  return geoP;
}

// ───────────────────────── per-item records ─────────────────────────

function pathOf(id: string): string {
  let p = paths.get(id);
  if (p === undefined) {
    try {
      p = decodeId(id);
    } catch {
      p = '';
    }
    paths.set(id, p);
  }
  return p;
}

function placeOf(meta: IndexMeta | null): { place: PlaceRef | null; done: boolean } {
  if (!meta || !isValidGps(meta.lat, meta.lon)) return { place: null, done: true };
  if (!geo) return { place: null, done: false };
  const m = geo.nearest(meta.lat as number, meta.lon as number);
  if (!m) return { place: null, done: true };
  return { place: { key: m.key, name: m.admin1 ?? m.city ?? m.cc, city: m.city || null, cc: m.cc }, done: true };
}

function makeRec(it: MediaItem, meta: IndexMeta | null): Rec {
  const cam = meta ? normalizeCamera(meta.make, meta.model) : null;
  const flags = classify({
    name: it.name,
    dir: it.dir,
    ext: it.ext,
    kind: it.kind,
    width: it.width,
    height: it.height,
    make: meta?.make,
    model: meta?.model,
    lens: meta?.lens,
    gpano: !!meta && (meta.hint & INDEX_HINT.gpano) !== 0,
    pages: meta && (meta.hint & INDEX_HINT.animated) !== 0 ? 2 : 1,
  });
  const { place, done } = placeOf(meta);
  return { mtime: it.mtime, meta, flags, place, placeDone: done, cam: cam?.key && cam.model ? { key: cam.key, make: cam.make, model: cam.model } : null };
}

/** cached record (synchronous; items not prepared yet get a name/size-only record) */
function recOf(it: MediaItem): Rec {
  let r = recs.get(it.id);
  if (r && r.mtime === it.mtime) {
    if (!r.placeDone && geo) {
      const p = placeOf(r.meta);
      r.place = p.place;
      r.placeDone = p.done;
    }
    return r;
  }
  r = makeRec(it, null);
  recs.set(it.id, r);
  return r;
}

let lastQuery = { done: -1, at: 0 };

/** load geo + native metadata for these items (incremental); returns the native indexing status */
export async function prepareExplore(items: MediaItem[]): Promise<IndexStatus | null> {
  await loadGeo();
  const status = await indexStatus();
  const missing = new Map<string, MediaItem>();
  for (const it of items) {
    const r = recs.get(it.id);
    if (!r || r.mtime !== it.mtime || !r.meta) {
      const p = pathOf(it.id);
      if (p) missing.set(p, it);
    }
  }
  if (items.length) {
    // one background pass per session over the folders these items live in
    const all = new Set<string>();
    for (const it of items) {
      const p = pathOf(it.id);
      if (p) all.add(p);
    }
    await ensureIndexed(rootsOf(all), status);
  }
  const fresh = status ? status.done !== lastQuery.done : Date.now() - lastQuery.at > 60_000;
  let changed = false;
  if (missing.size && (fresh || Date.now() - lastQuery.at > 60_000)) {
    lastQuery = { done: status?.done ?? -1, at: Date.now() };
    const found = await queryIndex(rootsOf(missing.keys()));
    if (found) {
      for (const [p, meta] of found) {
        const it = missing.get(p);
        if (!it) continue;
        recs.set(it.id, makeRec(it, meta));
        changed = true;
      }
    }
  }
  // records built before geo was ready / never built
  for (const it of items) {
    const r = recs.get(it.id);
    if (!r || r.mtime !== it.mtime) {
      recs.set(it.id, makeRec(it, null));
      changed = true;
    } else if (!r.placeDone && geo) {
      recOf(it);
      changed = true;
    }
  }
  if (changed) version++;
  return status;
}

// ───────────────────────── Explore result ─────────────────────────

function signature(items: MediaItem[]): string {
  let h = 0;
  for (const it of items) {
    h = (h * 31 + it.mtime) | 0;
    for (let k = 0; k < it.id.length; k += 3) h = (h * 31 + it.id.charCodeAt(k)) | 0;
  }
  return `${items.length}:${h}`;
}

/** device wall clock encoded as UTC (same convention as local.ts localAt) */
function nowLocal(now: number): number {
  return now - new Date(now).getTimezoneOffset() * 60_000;
}

let memo: { sig: string; result: ExploreResult } | null = null;

const TYPE_TESTS: [MediaFlagName, (it: MediaItem, r: Rec) => boolean][] = [
  ['video', (it) => it.kind === 'video'],
  ['selfie', (_, r) => (r.flags & MEDIA_FLAG.selfie) !== 0],
  ['screenshot', (_, r) => (r.flags & MEDIA_FLAG.screenshot) !== 0],
  ['panorama', (_, r) => (r.flags & MEDIA_FLAG.panorama) !== 0],
  ['animated', (_, r) => (r.flags & MEDIA_FLAG.animated) !== 0],
];

/** pure grouping over prepared records */
export function computeExplore(items: MediaItem[], now = Date.now()): Omit<ExploreResult, 'indexing'> {
  const rows = items.filter((i) => i.kind === 'image' || i.kind === 'video').sort((a, b) => b.localAt - a.localAt || (a.id < b.id ? 1 : -1));
  const R = rows.map(recOf);

  const otd = onThisDay(
    rows.map((r) => ({ id: r.id, localAt: r.localAt })),
    nowLocal(now),
  );
  const trips = findTrips(
    rows.map((it, i) => {
      const r = R[i];
      return { id: it.id, localAt: it.localAt, lat: r.meta?.lat, lon: r.meta?.lon, placeKey: r.place?.key ?? null, placeName: r.place?.name ?? null, cc: r.place?.cc ?? null };
    }),
  ).slice(0, MAX_TRIPS);

  const places = new Map<string, { g: PlaceGroup; lat: number; lon: number; cities: Map<string, number> }>();
  rows.forEach((it, i) => {
    const r = R[i];
    if (!r.place || !r.meta) return;
    let p = places.get(r.place.key);
    if (!p) places.set(r.place.key, (p = { g: { key: r.place.key, cc: r.place.cc, name: r.place.name, city: null, count: 0, coverId: it.id, lat: 0, lon: 0 }, lat: 0, lon: 0, cities: new Map() }));
    p.g.count++;
    p.lat += r.meta.lat as number;
    p.lon += r.meta.lon as number;
    if (r.place.city) p.cities.set(r.place.city, (p.cities.get(r.place.city) ?? 0) + 1);
  });
  const placeList = [...places.values()].map((p) => {
    let city: string | null = null;
    let best = 0;
    for (const [c, n] of p.cities) if (n > best) [city, best] = [c, n];
    return { ...p.g, city, lat: Math.round((p.lat / p.g.count) * 1e5) / 1e5, lon: Math.round((p.lon / p.g.count) * 1e5) / 1e5 };
  });
  placeList.sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));

  const types: TypeGroup[] = [];
  for (const [flag, test] of TYPE_TESTS) {
    let count = 0;
    let coverId = '';
    rows.forEach((it, i) => {
      if (!test(it, R[i])) return;
      if (!count) coverId = it.id;
      count++;
    });
    if (count) types.push({ flag, count, coverId });
  }

  const cams = new Map<string, CameraGroup>();
  rows.forEach((it, i) => {
    const c = R[i].cam;
    if (!c) return;
    const g = cams.get(c.key);
    if (g) g.count++;
    else cams.set(c.key, { key: c.key, make: c.make, model: c.model, count: 1, coverId: it.id });
  });
  const cameras = [...cams.values()].sort((a, b) => b.count - a.count || a.model.localeCompare(b.model));

  const years = new Map<number, YearGroup>();
  for (const it of rows) {
    const y = new Date(it.localAt).getUTCFullYear();
    const g = years.get(y);
    if (g) g.count++;
    else years.set(y, { year: y, count: 1, coverId: it.id });
  }

  return { memories: [...otd, ...trips], places: placeList, types, cameras, years: [...years.values()].sort((a, b) => b.year - a.year) };
}

export async function buildExplore(items: MediaItem[], now = Date.now()): Promise<ExploreResult> {
  const media = items.filter((i) => i.kind === 'image' || i.kind === 'video');
  const status = await prepareExplore(media);
  const indexing = status && status.total > 0 ? { done: Math.min(status.done, status.total), total: status.total } : { done: 0, total: 0 };
  const sig = `${signature(media)}|${version}|${localDayKey(nowLocal(now))}`;
  if (memo?.sig !== sig) memo = { sig, result: { ...computeExplore(media, now), indexing } };
  return { ...memo.result, indexing };
}

// ───────────────────────── browse filters / map points ─────────────────────────

function matchesSearch(it: MediaItem, r: Rec, terms: string[]): boolean {
  if (!terms.length) return true;
  const hay = foldText([it.name, r.place?.city, r.place?.name, r.place?.cc, r.cam?.make, r.cam?.model].filter(Boolean).join(' '));
  return terms.every((t) => hay.includes(t));
}

function flagMatch(it: MediaItem, r: Rec, flag: MediaFlagName): boolean {
  return flag === 'video' ? it.kind === 'video' : (r.flags & MEDIA_FLAG[flag]) !== 0;
}

/**
 * Apply the Explore filters of a BrowseQuery: place (`TR-34` or bare `TR`), camera key, flag,
 * kind 'image-video', and — when local.ts hands over the raw free text — a parseSearch()-based q.
 * Synchronous: uses the records prepared by buildExplore() (missing ones are classified on the fly).
 */
export function filterExplore(items: MediaItem[], q: BrowseQuery): MediaItem[] {
  const imageVideo = (q.kind as string) === 'image-video';
  const search = q.q ? parseSearch(q.q) : null;
  if (!q.place && !q.camera && !q.flag && !imageVideo && !search) return items;
  const place = q.place?.toUpperCase();
  const bareCc = !!place && !place.includes('-');
  const camera = q.camera?.toLowerCase();
  return items.filter((it) => {
    if (imageVideo && it.kind !== 'image' && it.kind !== 'video') return false;
    const r = recOf(it);
    if (it.flags !== r.flags) it.flags = r.flags;
    if (q.flag && !flagMatch(it, r, q.flag)) return false;
    if (place && (bareCc ? r.place?.cc !== place : r.place?.key.toUpperCase() !== place)) return false;
    if (camera && r.cam?.key !== camera) return false;
    if (search) {
      if (search.flag && !flagMatch(it, r, search.flag)) return false;
      if (search.from !== undefined && it.localAt < search.from) return false;
      if (search.to !== undefined && it.localAt >= search.to) return false;
      if (search.month !== undefined && new Date(it.localAt).getUTCMonth() + 1 !== search.month) return false;
      if (!matchesSearch(it, r, search.terms)) return false;
    }
    return true;
  });
}

export function geoPointsFor(items: MediaItem[]): GeoPoints {
  const points: GeoPoints['points'] = [];
  for (const it of items) {
    if (it.kind !== 'image' && it.kind !== 'video') continue;
    const m = recOf(it).meta;
    if (m && isValidGps(m.lat, m.lon)) points.push([it.id, Math.round((m.lat as number) * 1e5) / 1e5, Math.round((m.lon as number) * 1e5) / 1e5]);
  }
  return { points };
}

/** test hooks */
export function _setGeoLoader(fn: () => Promise<Uint8Array>) {
  geoLoader = fn;
  geoP = null;
  geo = null;
}
export function _resetExplore() {
  recs.clear();
  paths.clear();
  memo = null;
  version = 0;
  lastQuery = { done: -1, at: 0 };
}
