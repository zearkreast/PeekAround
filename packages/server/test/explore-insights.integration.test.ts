import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import type { BrowseResult, CalendarYear, ExploreResult, MediaDetails, MediaItem, MetaStatus } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import { _resetInsightFill } from '../src/insights';
import { META_V } from '../src/media/metadata';

const L = (iso: string) => Date.parse(`${iso}Z`);
const dms = (v: number) => {
  const a = Math.abs(v);
  const d = Math.floor(a);
  const m = Math.floor((a - d) * 60);
  const s = Math.round(((a - d) * 60 - m) * 60 * 100);
  return `${d}/1 ${m}/1 ${s}/100`;
};
const exifDate = (local: number) => new Date(local).toISOString().slice(0, 19).replace('T', ' ').replace(/-/g, ':');

async function jpg(file: string, o: { w?: number; h?: number; color?: string; local?: number; off?: string; gps?: [number, number]; xmp?: string; seed?: number } = {}) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const w = o.w ?? 120;
  const h = o.h ?? 90;
  let img = sharp({ create: { width: w, height: h, channels: 3, background: o.color ?? '#3b7' } });
  if (o.seed !== undefined) {
    // distinct content per file (so only real copies are duplicates)
    const raw = Buffer.alloc(w * h * 3);
    const c = parseInt((o.color ?? '#33bb77').slice(1).padEnd(6, '0'), 16);
    for (let i = 0; i < w * h; i++) {
      const j = ((i * 2654435761 + o.seed * 97) >>> 0) % 9;
      raw[i * 3] = Math.min(255, ((c >> 16) & 255) + j);
      raw[i * 3 + 1] = Math.min(255, ((c >> 8) & 255) + j);
      raw[i * 3 + 2] = Math.min(255, (c & 255) + j);
    }
    img = sharp(raw, { raw: { width: w, height: h, channels: 3 } });
  }
  const exif: Record<string, Record<string, string>> = { IFD0: { Make: 'Apple', Model: 'iPhone 13' }, IFD2: {} };
  if (o.local !== undefined) exif.IFD2.DateTimeOriginal = exifDate(o.local);
  if (o.off) exif.IFD2.OffsetTimeOriginal = o.off;
  if (o.gps) exif.IFD3 = { GPSLatitudeRef: o.gps[0] < 0 ? 'S' : 'N', GPSLatitude: dms(o.gps[0]), GPSLongitudeRef: o.gps[1] < 0 ? 'W' : 'E', GPSLongitude: dms(o.gps[1]) };
  img = img.withExif(exif);
  if (o.xmp) img = img.withXmp(o.xmp);
  await img.jpeg({ quality: 80 }).toFile(file);
}

const xmpGps = (lat: string, lon: string) =>
  `<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/" exif:GPSLatitude="${lat}" exif:GPSLongitude="${lon}"/></rdf:RDF></x:xmpmeta><?xpacket end="w"?>`;

const side = (dir: string, json: string, lat: number, lon: number, ts: number) =>
  fs.writeFileSync(path.join(dir, json), JSON.stringify({ title: 'x', photoTakenTime: { timestamp: String(ts) }, geoData: { latitude: lat, longitude: lon, altitude: 0 } }));

describe('explore insights, metadata sources, duplicates, diagnostics', () => {
  let srv: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-insights-'));
  const lib = path.join(tmp, 'lib');
  const get = async <T>(url: string): Promise<T> => {
    const r = await srv.app.inject(url);
    if (r.statusCode !== 200) throw new Error(`${url}: ${r.statusCode} ${r.body}`);
    return r.json() as T;
  };
  const all = async () => (await get<BrowseResult>('/api/browse?recursive=1&folderId=1&limit=5000')).items;
  const byName = async (name: string) => (await all()).find((i) => i.name === name)!;
  const total = async (qs: string) => (await get<BrowseResult>(`/api/browse?${qs}&limit=1`)).total;
  const settle = async (pred: () => boolean = () => true) => {
    for (let i = 0; i < 400; i++) {
      const c = srv.ctx.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE meta_state = 0 OR (thumb_state = 0 AND kind IN ('image','video'))")!.c;
      if (!c && !srv.ctx.scanner.busy && !srv.ctx.scanner.enriching && !srv.ctx.explore.dups.busy && pred()) return;
      await new Promise((res) => setTimeout(res, 100));
    }
    throw new Error('library did not settle');
  };

  beforeAll(async () => {
    // a year with ≥ 12 photos (year in review), golden hour + night in Istanbul (EXIF offset known)
    for (let k = 0; k < 14; k++) {
      await jpg(path.join(lib, '2023', `p${k}.jpg`), {
        seed: k,
        w: k % 3 === 0 ? 90 : 120,
        h: k % 3 === 0 ? 120 : 90,
        local: L(`2023-${String(1 + (k % 12)).padStart(2, '0')}-10T${k % 2 ? '12' : '20'}:15:00`),
        off: '+03:00',
        gps: k % 4 === 3 ? undefined : [41.0082 + k * 0.001, 28.9784],
        color: k % 2 ? '#2244dd' : '#dd2222',
      });
    }
    // golden hour: Istanbul sunset 2024-06-21 at 20:39 (+03:00)
    await jpg(path.join(lib, '2024', 'sunset.jpg'), { seed: 101, local: L('2024-06-21T20:30:00'), off: '+03:00', gps: [41.0082, 28.9784], color: '#ee8822' });
    await jpg(path.join(lib, '2024', 'square.jpg'), { seed: 102, w: 100, h: 100, local: L('2024-06-22T13:00:00'), off: '+03:00', gps: [43.7696, 11.2558] });
    // XMP-only GPS (no EXIF GPS block)
    await jpg(path.join(lib, '2024', 'xmp-only.jpg'), { seed: 103, local: L('2024-05-01T10:00:00'), off: '+03:00', xmp: xmpGps('41,0.738N', '28,58.476E') });
    // messaging app (no metadata)
    await sharp({ create: { width: 80, height: 100, channels: 3, background: '#888' } }).jpeg().toFile(path.join(lib, 'IMG-20240101-WA0001.jpg'));
    // Google Takeout: EXIF stripped, JSON sidecars next to the files
    const tk = path.join(lib, 'Takeout', 'Google Photos', 'Photos from 2019');
    fs.mkdirSync(tk, { recursive: true });
    await sharp({ create: { width: 90, height: 70, channels: 3, background: '#191' } }).jpeg().toFile(path.join(tk, 'IMG_20190704_101010.jpg'));
    side(tk, 'IMG_20190704_101010.jpg.json', 37.9715, 23.7267, 1562235010);
    await sharp({ create: { width: 90, height: 70, channels: 3, background: '#119' } }).jpeg().toFile(path.join(tk, 'IMG_20190705_121212345_HDR.jpg'));
    side(tk, 'IMG_20190705_121212345_HDR.jpg.supplemental-me.json', 37.4467, 25.3289, 1562328732);
    await sharp({ create: { width: 91, height: 70, channels: 3, background: '#911' } }).jpeg().toFile(path.join(tk, 'IMG_1234(1).jpg'));
    side(tk, 'IMG_1234.jpg.supplemental-metadata(1).json', 36.3932, 25.4615, 1562415132);
    await sharp({ create: { width: 92, height: 70, channels: 3, background: '#991' } }).jpeg().toFile(path.join(tk, 'IMG_1234-edited.jpg'));
    side(tk, 'IMG_1234.jpg.json', 36.4, 25.47, 1562415100);
    // exact duplicates: two copies of p1, one of p2 (+ a same-size but different file)
    fs.mkdirSync(path.join(lib, 'Yedek'), { recursive: true });
    fs.copyFileSync(path.join(lib, '2023', 'p1.jpg'), path.join(lib, 'Yedek', 'p1.jpg'));
    fs.copyFileSync(path.join(lib, '2023', 'p1.jpg'), path.join(lib, 'Yedek', 'p1 kopya.jpg'));
    fs.copyFileSync(path.join(lib, '2023', 'p2.jpg'), path.join(lib, 'Yedek', 'p2.jpg'));
    const p4 = fs.readFileSync(path.join(lib, '2023', 'p4.jpg'));
    const twin = Buffer.from(p4);
    twin[twin.length - 3] ^= 0xff; // same size, different bytes
    fs.writeFileSync(path.join(lib, 'Yedek', 'p4-twin.jpg'), twin);

    srv = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false });
    await srv.app.inject({ method: 'PATCH', url: '/api/settings', payload: { homeTimeZone: 'Europe/Istanbul' } });
    await settle();
    // duplicates are hashed lazily once Explore is asked
    await get<ExploreResult>('/api/explore');
    await settle(() => srv.ctx.explore.dups.pending() === 0);
    srv.ctx.explore.invalidate(false);
  }, 180_000);

  afterAll(async () => {
    await srv?.close();
  });

  it('reads XMP GPS strings and Google Takeout sidecars', async () => {
    const x = await get<MediaDetails>(`/api/media/${(await byName('xmp-only.jpg')).id}`);
    expect(x.gps).toEqual({ lat: expect.closeTo(41.0123, 3), lon: expect.closeTo(28.9746, 3) });
    expect(x.place).toMatchObject({ cc: 'TR', admin1: 'İstanbul' });

    const t1 = await byName('IMG_20190704_101010.jpg');
    expect(t1.takenAt).toBe(1562235010000);
    expect((await get<MediaDetails>(`/api/media/${t1.id}`)).gps).toEqual({ lat: 37.9715, lon: 23.7267 });
    const t2 = await get<MediaDetails>(`/api/media/${(await byName('IMG_20190705_121212345_HDR.jpg')).id}`);
    expect(t2.gps).toEqual({ lat: 37.4467, lon: 25.3289 });
    expect(t2.takenAt).toBe(1562328732000);
    const t3 = await get<MediaDetails>(`/api/media/${(await byName('IMG_1234(1).jpg')).id}`);
    expect(t3.gps).toEqual({ lat: 36.3932, lon: 25.4615 });
    const t4 = await get<MediaDetails>(`/api/media/${(await byName('IMG_1234-edited.jpg')).id}`);
    expect(t4.gps).toEqual({ lat: 36.4, lon: 25.47 });
    expect(t4.place?.cc).toBe('GR');
    // all rows carry the new metadata version
    expect(srv.ctx.db.get<{ c: number }>('SELECT count(*) c FROM media WHERE meta_v < ?', META_V)!.c).toBe(0);
  });

  it('explore: stats, year in review, countries/cities, time of day, colours, shapes, rankings', async () => {
    _resetInsightFill();
    srv.ctx.explore.invalidate(false);
    const r = await get<ExploreResult>('/api/explore');
    const n = (await all()).filter((i) => i.kind === 'image' || i.kind === 'video').length;
    expect(r.stats!.photos + r.stats!.videos).toBe(n);
    expect(r.stats!.bytes).toBe((await all()).reduce((a, i) => a + i.size, 0));
    expect(r.stats).toMatchObject({ countries: 3, firstYear: 2019 });
    expect(r.countries!.map((c) => c.cc)).toEqual(expect.arrayContaining(['TR', 'IT', 'GR']));
    expect(r.cities!.some((c) => c.cc === 'TR' && c.placeKey === 'TR-34')).toBe(true);
    const y23 = r.yearReviews!.find((m) => m.id === 'year-2023')!;
    expect(y23).toMatchObject({ kind: 'year', count: 14 + 4 }); // + the copies in Yedek (same EXIF date)
    expect(y23.months!.filter(Boolean).length).toBe(12);
    expect(r.calendarYears![0]).toBeGreaterThanOrEqual(2024);
    // night (20:15 in Istanbul in winter) and golden hour (sunset)
    const night = r.dayTimes!.find((g) => g.value === 'night')!;
    const golden = r.dayTimes!.find((g) => g.value === 'golden')!;
    expect(night.count).toBeGreaterThan(0);
    expect(golden.coverIds).toContain((await byName('sunset.jpg')).id);
    expect(r.colors!.map((g) => g.value)).toEqual(expect.arrayContaining(['red', 'blue']));
    expect(r.orientations!.find((g) => g.value === 'square')!.coverIds).toContain((await byName('square.jpg')).id);
    expect(r.orientations!.find((g) => g.value === 'portrait')!.count).toBeGreaterThanOrEqual(5);
    expect(r.largest![0].value).toBeGreaterThanOrEqual(r.largest![1].value);
    expect(r.noLocation).toBe(srv.ctx.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE lat IS NULL AND kind IN ('image','video')")!.c);
    expect(r.noLocation).toBeGreaterThanOrEqual(2);
    expect(r.duplicates).toMatchObject({ groups: 2, files: 5, pending: 0 });
    expect(r.duplicates!.wasted).toBe(2 * fs.statSync(path.join(lib, '2023', 'p1.jpg')).size + fs.statSync(path.join(lib, '2023', 'p2.jpg')).size);
  });

  it('browse filters behind the result pages', async () => {
    const r = await get<ExploreResult>('/api/explore');
    expect(await total('gps=none')).toBe(r.noLocation);
    expect(await total('gps=any')).toBe(r.stats!.withLocation);
    const red = r.colors!.find((g) => g.value === 'red')!;
    expect(await total('color=red')).toBe(red.count);
    expect(await total('color=nope')).toBe(await total('x=1')); // unknown value = no filter
    for (const g of r.orientations!) expect(await total(`orient=${g.value}`), g.value).toBe(g.count);
    for (const g of r.dayTimes!) expect(await total(`daytime=${g.value}`), g.value).toBe(g.count);
    expect(await total('dup=1')).toBe(5);
    const dupPage = await get<BrowseResult>('/api/browse?dup=1&limit=10');
    // copies are next to each other
    const sizes = dupPage.items.map((i) => i.size);
    expect([...sizes].sort((a, b) => b - a)).toEqual(sizes);
    const c = r.cities!.find((x) => x.placeKey === 'TR-34')!;
    expect(await total(`place=TR-34&city=${encodeURIComponent(c.name)}`)).toBe(c.count);
    expect(await total(`vlen=short`)).toBe(0);
    // skeleton follows the same filters
    const sk = await get<{ total: number }>('/api/browse/skeleton?gps=none');
    expect(sk.total).toBe(r.noLocation);
    // skeleton ↔ pages parity for every insight filter (same rows, same order)
    for (const qs of ['gps=none', 'gps=any', 'color=red', 'orient=portrait', 'orient=landscape', 'daytime=golden', 'daytime=night', 'vlen=short', 'dup=1', 'city=x', 'byAdded=1']) {
      const s2 = await get<{ total: number; aspects: string; days: { count: number }[] | null }>(`/api/browse/skeleton?${qs}`);
      const p2 = await get<BrowseResult>(`/api/browse?${qs}&limit=5000`);
      expect(s2.total, qs).toBe(p2.total);
      expect(Buffer.from(s2.aspects, 'base64').length, qs).toBe(p2.items.length);
      // rows that are not in capture-time order carry no day groups (they would split / repeat days)
      if (qs === 'dup=1' || qs === 'byAdded=1') expect(s2.days, qs).toBeNull();
      else expect(s2.days?.reduce((n, d) => n + d.count, 0) ?? 0, qs).toBe(p2.total);
    }
  });

  it('recently added: newest additions first (falls back to mtime for old rows)', async () => {
    await jpg(path.join(lib, 'new', 'fresh.jpg'), { seed: 999, local: L('2010-01-01T10:00:00') });
    await srv.ctx.scanner.scanFolder(1);
    await settle();
    const page = await get<BrowseResult>('/api/browse?byAdded=1&recursive=1&sort=mtime&limit=3');
    expect(page.items[0].name).toBe('fresh.jpg');
    srv.ctx.explore.invalidate(false);
    const r = await get<ExploreResult>('/api/explore');
    expect(r.recent![0].id).toBe(page.items[0].id);
  });

  it('calendar heatmap counts per day', async () => {
    const cal = await get<CalendarYear>('/api/explore/calendar?year=2023');
    expect(cal.days).toHaveLength(365);
    expect(cal.total).toBe(18);
    // p1 + its two copies + p13: 2023-02-10
    const d = Math.round((L('2023-02-10T00:00:00') - L('2023-01-01T00:00:00')) / 86_400_000);
    expect(cal.days[d]).toBe(4);
    expect(cal.max).toBe(4);
    expect((await srv.app.inject('/api/explore/calendar?year=abc')).statusCode).toBe(400);
  });

  it('duplicate groups with their items and folders', async () => {
    const d = await get<{ groups: { hash: string; size: number; ids: string[] }[]; total: number; items: MediaItem[]; paths: Record<string, string>; pending: number }>('/api/explore/duplicates');
    expect(d.total).toBe(2);
    expect(d.groups[0].ids).toHaveLength(3);
    expect(d.items).toHaveLength(5);
    const names = d.groups[0].ids.map((id) => d.items.find((i) => i.id === id)!.name).sort();
    expect(names).toEqual(['p1 kopya.jpg', 'p1.jpg', 'p1.jpg']);
    expect(Object.values(d.paths)).toEqual(expect.arrayContaining(['lib/2023', 'lib/Yedek']));
    // a changed file leaves its group (content hash is reset by the trigger)
    const copy = path.join(lib, 'Yedek', 'p2.jpg');
    fs.appendFileSync(copy, Buffer.from([0]));
    await srv.ctx.scanner.upsertFile(1, 'Yedek/p2.jpg');
    await settle();
    expect(srv.ctx.db.get<{ chash: string | null }>("SELECT chash FROM media WHERE name = 'p2.jpg' AND dir = 'Yedek'")!.chash).toBeNull();
    const d2 = await get<{ total: number }>('/api/explore/duplicates');
    expect(d2.total).toBe(1);
  });

  it('metadata diagnostics + re-read (e.g. sidecars added later)', async () => {
    const s = await get<MetaStatus>('/api/meta/status');
    const l = s.libraries[0];
    expect(l.name).toBe('Lib');
    expect(l.withLocation + l.withoutLocation + l.pending + l.unreadable).toBe(l.total);
    expect(l.messaging).toBe(1);
    expect(l.pending).toBe(0);

    // the user drops a Takeout JSON next to an already indexed file
    const wa = path.join(lib, 'IMG-20240101-WA0001.jpg');
    fs.writeFileSync(`${wa}.json`, JSON.stringify({ photoTakenTime: { timestamp: '1704100000' }, geoData: { latitude: 39.9334, longitude: 32.8597 } }));
    const before = await get<MediaDetails>(`/api/media/${(await byName('IMG-20240101-WA0001.jpg')).id}`);
    expect(before.gps).toBeNull();
    const r = await srv.app.inject({ method: 'POST', url: '/api/meta/reread', payload: { libraryId: '1' } });
    expect(r.statusCode).toBe(200);
    expect(r.json().queued).toBeGreaterThan(10);
    await settle(() => srv.ctx.db.get<{ c: number }>('SELECT count(*) c FROM media WHERE meta_v < ?', META_V)!.c === 0);
    const after = await get<MediaDetails>(`/api/media/${(await byName('IMG-20240101-WA0001.jpg')).id}`);
    expect(after.gps).toEqual({ lat: 39.9334, lon: 32.8597 });
    expect(after.place?.cc).toBe('TR');
    const s2 = await get<MetaStatus>('/api/meta/status');
    expect(s2.libraries[0].withLocation).toBe(l.withLocation + 1);
    expect((await srv.app.inject({ method: 'POST', url: '/api/meta/reread', payload: { libraryId: '999' } })).statusCode).toBe(404);
  });
});
