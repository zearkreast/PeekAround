import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MediaItem } from '@peekaround/shared';

// native PeekIndex fake (columnar rows keyed by absolute path)
const native = vi.hoisted(() => ({
  rows: new Map<string, { lat: number | null; lon: number | null; make: string | null; model: string | null; lens: string | null; offset: number | null; flags: number }>(),
  status: { done: 0, total: 0, running: false },
  queries: [] as string[][],
  syncs: [] as string[][],
  missing: false,
}));

vi.mock('./plugin', () => ({
  PeekLocal: {
    async indexQuery({ paths }: { paths: string[] }) {
      if (native.missing) throw new Error('"PeekLocal.indexQuery()" is not implemented on android');
      native.queries.push(paths);
      const cols = { path: [] as string[], lat: [] as (number | null)[], lon: [] as (number | null)[], make: [] as (string | null)[], model: [] as (string | null)[], lens: [] as (string | null)[], offset: [] as (number | null)[], flags: [] as number[] };
      for (const [p, r] of native.rows) {
        if (!paths.some((root) => p === root || p.startsWith(`${root}/`))) continue;
        cols.path.push(p);
        cols.lat.push(r.lat);
        cols.lon.push(r.lon);
        cols.make.push(r.make);
        cols.model.push(r.model);
        cols.lens.push(r.lens);
        cols.offset.push(r.offset);
        cols.flags.push(r.flags);
      }
      return cols;
    },
    async indexStatus() {
      if (native.missing) throw new Error('"PeekLocal.indexStatus()" is not implemented on android');
      return native.status;
    },
    async indexSync({ paths }: { paths: string[] }) {
      native.syncs.push(paths);
    },
  },
}));

const { buildExplore, filterExplore, geoPointsFor, _setGeoLoader, _resetExplore } = await import('./explore');
const { rootsOf, decodeId, parseColumns, _resetIndexBridge } = await import('./indexBridge');

const GEO = fileURLToPath(new URL('../../../public/geo/cities.bin', import.meta.url));

function encId(path: string): string {
  return Buffer.from(path, 'utf8').toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

const ROOT = '/storage/emulated/0/DCIM';
const D = (y: number, m: number, d: number, h = 12) => Date.UTC(y, m - 1, d, h);

function item(rel: string, localAt: number, extra: Partial<MediaItem> = {}): MediaItem {
  const name = rel.slice(rel.lastIndexOf('/') + 1);
  const ext = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
  const kind = ['mp4', 'mov'].includes(ext) ? 'video' : ext === 'mp3' ? 'audio' : 'image';
  return {
    id: encId(`${ROOT}/${rel}`),
    folderId: 'f1',
    dir: rel.includes('/') ? rel.slice(0, rel.lastIndexOf('/')) : '',
    name,
    ext,
    kind,
    size: 1000,
    mtime: localAt,
    width: 4000,
    height: 3000,
    duration: null,
    takenAt: localAt,
    localAt,
    flags: 0,
    thumbhash: null,
    favorite: false,
    playback: 'direct',
    ...extra,
  } as MediaItem;
}

function gps(rel: string, lat: number, lon: number, make = 'samsung', model = 'SM-G991B', lens: string | null = null, flags = 0) {
  native.rows.set(`${ROOT}/${rel}`, { lat, lon, make, model, lens, offset: 180, flags });
}

const NOW = D(2026, 11, 5, 10);

function library(): MediaItem[] {
  const list: MediaItem[] = [];
  // home: İstanbul, many days through 2024 (→ home of the year)
  for (let d = 0; d < 20; d++) {
    const rel = `Camera/ist_${d}.jpg`;
    list.push(item(rel, D(2024, 2, 1 + d) + d * 9 * 86_400_000));
    gps(rel, 41.0082, 28.9784);
  }
  // trip: Antalya 2–5 Jul 2024
  for (let d = 0; d < 4; d++)
    for (let k = 0; k < 3; k++) {
      const rel = `Camera/ant_${d}_${k}.jpg`;
      list.push(item(rel, D(2024, 7, 2 + d, 9 + k)));
      gps(rel, 36.8969, 30.7133, 'NIKON CORPORATION', 'NIKON D750');
    }
  // on this day (Nov 2025, 1 year ago)
  for (let k = 0; k < 4; k++) list.push(item(`Camera/otd_${k}.jpg`, D(2025, 11, 4 + (k % 2), 10 + k)));
  // types
  list.push(item('Screenshots/Screenshot_20250101-101500.png', D(2025, 1, 1)));
  list.push(item('Camera/selfie.jpg', D(2025, 2, 1)));
  gps('Camera/selfie.jpg', 41.0082, 28.9784, 'samsung', 'SM-G991B', 'Front camera');
  list.push(item('Camera/pano.jpg', D(2025, 3, 1), { width: 9000, height: 2000 }));
  gps('Camera/pano.jpg', 41.0082, 28.9784, 'samsung', 'SM-G991B', null, 1);
  list.push(item('Camera/clip.mp4', D(2025, 4, 1)));
  list.push(item('Camera/funny.gif', D(2025, 5, 1)));
  list.push(item('Music/song.mp3', D(2025, 6, 1)));
  return list;
}

beforeEach(() => {
  native.rows.clear();
  native.queries.length = 0;
  native.syncs.length = 0;
  native.status = { done: 10, total: 10, running: false };
  native.missing = false;
  _resetExplore();
  _resetIndexBridge();
  _setGeoLoader(async () => new Uint8Array(readFileSync(GEO)));
});

describe('android local explore', () => {
  it('helpers: ids decode to paths, roots collapse nested dirs, columns parse', () => {
    const p = `${ROOT}/Düğün/İstanbul ş.jpg`;
    expect(decodeId(encId(p))).toBe(p);
    expect(rootsOf(['/a/b/1.jpg', '/a/b/c/2.jpg', '/a/bb/3.jpg', '/x/4.jpg'])).toEqual(['/a/b', '/a/bb', '/x']);
    const m = parseColumns({ path: ['/a.jpg'], lat: [41], lon: [29], make: [''], model: ['X'], lens: [null], offset: [null], flags: [2] });
    expect(m.get('/a.jpg')).toEqual({ lat: 41, lon: 29, make: null, model: 'X', lens: null, offset: null, hint: 2 });
    expect(parseColumns(null).size).toBe(0);
  });

  it('builds places, trips, on-this-day, types, cameras and years from native metadata', async () => {
    const items = library();
    const r = await buildExplore(items, NOW);
    // places: İstanbul (TR-34) biggest, Antalya (TR-07)
    expect(r.places.map((p) => [p.key, p.name, p.cc])).toEqual([
      ['TR-34', 'İstanbul', 'TR'],
      ['TR-07', 'Antalya', 'TR'],
    ]);
    expect(r.places[0].count).toBe(22);
    expect(r.places[1].count).toBe(12);
    // memories
    const otd = r.memories.find((m) => m.kind === 'onthisday');
    expect(otd).toMatchObject({ yearsAgo: 1, count: 4 });
    const trip = r.memories.find((m) => m.kind === 'trip');
    expect(trip).toMatchObject({ place: 'Antalya', cc: 'TR', from: Date.UTC(2024, 6, 2), to: Date.UTC(2024, 6, 6), count: 12 });
    // types (audio never counted)
    const types = Object.fromEntries(r.types.map((t) => [t.flag, t.count]));
    expect(types).toEqual({ video: 1, selfie: 1, screenshot: 1, panorama: 1, animated: 1 });
    // cameras normalised
    expect(r.cameras.map((c) => [c.key, c.make, c.model, c.count])).toEqual([
      ['samsung|sm-g991b', 'Samsung', 'SM-G991B', 22],
      ['nikon|d750', 'Nikon', 'D750', 12],
    ]);
    expect(r.years.map((y) => y.year)).toEqual([2025, 2024]);
    expect(r.indexing).toEqual({ done: 10, total: 10 });
    // one native query over the folder roots, one sync for the session
    expect(native.queries).toHaveLength(1);
    expect(native.queries[0]).toEqual([`${ROOT}/Camera`, `${ROOT}/Screenshots`]); // audio never asked for
    expect(native.syncs).toHaveLength(1);
  });

  it('filters by place / bare country / camera / flag / image-video and parsed search text', async () => {
    const items = library();
    await buildExplore(items, NOW);
    const names = (q: Parameters<typeof filterExplore>[1]) => filterExplore(library(), q).map((i) => i.name);
    expect(names({ place: 'TR-07' })).toHaveLength(12);
    expect(names({ place: 'tr' })).toHaveLength(34);
    expect(names({ camera: 'Nikon|D750' })).toHaveLength(12);
    expect(names({ flag: 'selfie' })).toEqual(['selfie.jpg']);
    expect(names({ flag: 'screenshot' })).toEqual(['Screenshot_20250101-101500.png']);
    expect(names({ flag: 'video' })).toEqual(['clip.mp4']);
    expect(names({ flag: 'animated' })).toEqual(['funny.gif']);
    expect(names({ kind: 'image-video' as never })).not.toContain('song.mp3');
    expect(names({ q: 'antalya 2024' })).toHaveLength(12);
    expect(names({ q: 'nikon temmuz' })).toHaveLength(12);
    expect(names({ q: 'video' })).toEqual(['clip.mp4']);
    // no explore filters → same array back (fast path)
    const all = library();
    expect(filterExplore(all, { sort: 'takenAt' })).toBe(all);
    // flags are filled in on filtered items
    expect(filterExplore(library(), { flag: 'panorama' })[0].flags).toBeGreaterThan(0);
  });

  it('insights the native index can feed: stats, countries/cities, shapes, rankings, insight filters', async () => {
    const items = library();
    const r = await buildExplore(items, NOW);
    expect(r.stats).toMatchObject({ photos: 40, videos: 1, countries: 1, withLocation: 34 });
    expect(r.countries).toEqual([expect.objectContaining({ cc: 'TR', count: 34, regions: 2 })]);
    expect(r.cities!.map((c) => c.placeKey)).toEqual(expect.arrayContaining(['TR-34', 'TR-07']));
    expect(r.noLocation).toBe(7);
    expect(r.orientations!.find((g) => g.value === 'landscape')!.count).toBeGreaterThan(30);
    expect(r.orientations!.find((g) => g.value === 'panorama')!.count).toBe(1);
    expect(r.largest).toHaveLength(20);
    expect(r.calendarYears).toEqual([2025, 2024]);
    expect(r.yearReviews!.map((m) => m.id)).toEqual(['year-2024']);
    // not computable locally → omitted
    expect(r.colors).toBeUndefined();
    expect(r.dayTimes).toBeUndefined();
    expect(r.duplicates).toBeUndefined();
    const names = (q: Parameters<typeof filterExplore>[1]) => filterExplore(library(), q).map((i) => i.name);
    expect(names({ gps: 'none', kind: 'image-video' as never }).sort()).toEqual(['Screenshot_20250101-101500.png', 'clip.mp4', 'funny.gif', 'otd_0.jpg', 'otd_1.jpg', 'otd_2.jpg', 'otd_3.jpg']);
    expect(names({ gps: 'any' })).toHaveLength(34);
    expect(names({ orient: 'panorama' })).toEqual(['pano.jpg']);
    expect(names({ color: 'red' })).toEqual([]);
    expect(names({ place: 'TR-07', city: r.cities!.find((c) => c.placeKey === 'TR-07')!.name })).toHaveLength(12);
    const recent = filterExplore(library(), { byAdded: true });
    expect(recent[0].mtime).toBe(Math.max(...library().map((i) => i.mtime)));
  });

  it('map points only for items with valid GPS', async () => {
    const items = library();
    await buildExplore(items, NOW);
    const pts = geoPointsFor(items).points;
    expect(pts).toHaveLength(34);
    expect(pts[0][1]).toBeCloseTo(41.0082, 4);
  });

  it('without the native index: dates, kinds and names only', async () => {
    native.missing = true;
    const r = await buildExplore(library(), NOW);
    expect(r.places).toEqual([]);
    expect(r.cameras).toEqual([]);
    expect(r.memories.filter((m) => m.kind === 'onthisday')).toHaveLength(1);
    // panorama by aspect + "pano" in the name, selfie by name
    expect(r.types.map((t) => t.flag).sort()).toEqual(['animated', 'panorama', 'screenshot', 'selfie', 'video']);
    expect(r.indexing).toEqual({ done: 0, total: 0 });
    expect(geoPointsFor(library()).points).toEqual([]);
  });

  it('memoises per item set and picks up metadata indexed later', async () => {
    native.status = { done: 0, total: 40, running: true };
    const items = library();
    const rows = new Map(native.rows);
    native.rows.clear();
    const a = await buildExplore(items, NOW);
    expect(a.places).toEqual([]);
    expect(a.indexing).toEqual({ done: 0, total: 40 });
    // native pass finished
    for (const [k, v] of rows) native.rows.set(k, v);
    native.status = { done: 40, total: 40, running: false };
    const b = await buildExplore(library(), NOW);
    expect(b.places.map((p) => p.key)).toEqual(['TR-34', 'TR-07']);
    const c = await buildExplore(library(), NOW);
    expect(c.places).toBe(b.places);
    expect(native.queries).toHaveLength(2);
  });

  it('is fast for 20k items', async () => {
    const many: MediaItem[] = [];
    for (let i = 0; i < 20_000; i++) {
      const rel = `Camera/${i}.jpg`;
      many.push(item(rel, D(2020, 1, 1) + i * 3_600_000));
      if (i % 2) gps(rel, 36 + (i % 50) / 10, 27 + (i % 70) / 10, 'Google', 'Pixel 7');
    }
    const t0 = performance.now();
    const r = await buildExplore(many, NOW);
    const t1 = performance.now();
    filterExplore(many, { place: r.places[0].key });
    const t2 = performance.now();
    expect(r.places.length).toBeGreaterThan(3);
    expect(t1 - t0).toBeLessThan(4000); // includes loading + decompressing the geo asset once
    expect(t2 - t1).toBeLessThan(200);
  });
});
