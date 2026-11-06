import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import {
  byteToAspect,
  instantToLocal,
  localDayKey,
  type BrowseResult,
  type BrowseSkeleton,
  type ExploreResult,
  type MediaDetails,
  type MediaItem,
  type ServerEvent,
} from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import { META_V } from '../src/media/metadata';

const MIN = 60_000;
const L = (iso: string) => Date.parse(`${iso}Z`);
const dms = (v: number) => {
  const a = Math.abs(v);
  const d = Math.floor(a);
  const m = Math.floor((a - d) * 60);
  const s = Math.round(((a - d) * 60 - m) * 60 * 100);
  return `${d}/1 ${m}/1 ${s}/100`;
};
const exifDate = (local: number) => new Date(local).toISOString().slice(0, 19).replace('T', ' ').replace(/-/g, ':');

interface Shot {
  rel: string;
  w: number;
  h: number;
  local?: number;
  off?: string;
  gps?: [number, number];
  gpsUtc?: number;
  make?: string;
  model?: string;
  lens?: string;
  fmt?: 'jpg' | 'png';
  mtime?: number;
}

async function shot(root: string, s: Shot) {
  const f = path.join(root, s.rel);
  fs.mkdirSync(path.dirname(f), { recursive: true });
  let img = sharp({ create: { width: s.w, height: s.h, channels: 3, background: '#4a7' } });
  if ((s.fmt ?? 'jpg') === 'jpg') {
    const exif: Record<string, Record<string, string>> = { IFD0: {}, IFD2: {} };
    if (s.local !== undefined) exif.IFD2.DateTimeOriginal = exifDate(s.local);
    if (s.off) exif.IFD2.OffsetTimeOriginal = s.off;
    if (s.make) exif.IFD0.Make = s.make;
    if (s.model) exif.IFD0.Model = s.model;
    if (s.lens) exif.IFD2.LensModel = s.lens;
    if (s.gps) {
      exif.IFD3 = { GPSLatitudeRef: 'N', GPSLatitude: dms(s.gps[0]), GPSLongitudeRef: 'E', GPSLongitude: dms(s.gps[1]) };
      if (s.gpsUtc) {
        const u = new Date(s.gpsUtc);
        exif.IFD3.GPSDateStamp = u.toISOString().slice(0, 10).replace(/-/g, ':');
        exif.IFD3.GPSTimeStamp = `${u.getUTCHours()}/1 ${u.getUTCMinutes()}/1 ${u.getUTCSeconds()}/1`;
      }
    }
    img = img.withExif(exif).jpeg({ quality: 70 });
  } else img = img.png();
  await img.toFile(f);
  if (s.mtime) fs.utimesSync(f, new Date(s.mtime), new Date(s.mtime));
}

const KADIKOY: [number, number] = [40.9903, 29.029];
const ANTALYA: [number, number] = [36.8841, 30.7056];

describe('grouping, skeleton and explore', () => {
  let srv: PeekServer;
  const events: ServerEvent[] = [];
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-group-'));
  const lib = path.join(tmp, 'lib');
  const get = async <T>(url: string): Promise<T> => {
    const r = await srv.app.inject(url);
    if (r.statusCode !== 200) throw new Error(`${url}: ${r.statusCode} ${r.body}`);
    return r.json() as T;
  };
  const byName = async (name: string) => (await get<BrowseResult>(`/api/browse?recursive=1&folderId=1&limit=5000`)).items.find((i) => i.name === name)!;

  beforeAll(async () => {
    await shot(lib, { rel: 'Tatil/selfie.jpg', w: 600, h: 800, local: L('2024-07-01T23:30:00'), off: '+03:00', gps: KADIKOY, make: 'samsung', model: 'SM-G991B', lens: 'Samsung Galaxy S21 front camera 2.2mm f/2.2' });
    // no OffsetTime, but GPS time → +03:00
    await shot(lib, { rel: 'Tatil/nikon.jpg', w: 900, h: 600, local: L('2024-07-03T10:00:00'), gps: ANTALYA, gpsUtc: Date.parse('2024-07-03T07:00:04Z'), make: 'NIKON CORPORATION', model: 'NIKON D750' });
    await shot(lib, { rel: 'Tatil/pano.jpg', w: 3000, h: 1000, local: L('2024-07-04T18:00:00'), off: '+03:00', gps: ANTALYA, make: 'samsung', model: 'SM-G991B' });
    await shot(lib, { rel: 'Screenshot_20240812-101500.png', w: 540, h: 1170, fmt: 'png', mtime: Date.parse('2024-08-12T07:15:00Z') });
    await shot(lib, { rel: 'plain.jpg', w: 400, h: 400, mtime: Date.parse('2023-03-03T12:00:00Z') });
    execFileSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc=size=160x120:rate=5', '-t', '1', path.join(lib, 'anim.gif')]);
    // random extra photos for the property test (some share timestamps to test id tie-breaks)
    let seed = 3;
    const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    for (let k = 0; k < 36; k++) {
      const local = L('2022-01-01T00:00:00') + Math.floor(rnd() * 1000) * 86_400_000 + (k % 4 === 0 ? 0 : Math.floor(rnd() * 86_400_000));
      await shot(lib, {
        rel: `Rastgele/${k % 3 ? 'a' : 'b'}/r${k}.jpg`,
        w: 300 + Math.floor(rnd() * 200),
        h: 300 + Math.floor(rnd() * 200),
        local,
        off: '+03:00',
        gps: k % 2 ? KADIKOY : k % 5 ? ANTALYA : undefined,
        make: k % 3 ? 'Apple' : 'samsung',
        model: k % 3 ? 'iPhone 13' : 'SM-G991B',
      });
    }
  }, 120_000);

  beforeAll(async () => {
    fs.mkdirSync(path.join(lib, 'Videolar'), { recursive: true });
    execFileSync('ffmpeg', [
      '-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x240:rate=10', '-t', '1', '-c:v', 'libx264', '-pix_fmt', 'yuv420p',
      '-metadata', 'creation_time=2025-12-31T21:30:00Z', '-metadata', 'location=+41.0422+029.0067/',
      '-metadata', 'com.android.manufacturer=Google', '-metadata', 'com.android.model=Pixel 7', '-movflags', 'use_metadata_tags',
      path.join(lib, 'Videolar', 'yilbasi.mp4'),
    ]);
    srv = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false });
    srv.ctx.events.on((e) => events.push(e));
    // home zone: Istanbul (the container itself runs in UTC)
    const r = await srv.app.inject({ method: 'PATCH', url: '/api/settings', payload: { homeTimeZone: 'Europe/Istanbul' } });
    expect(r.statusCode).toBe(200);
    const total = 36 + 7;
    // wait until every background pass is done (slow CI runners: the background budget is small there)
    for (let i = 0, quiet = 0; i < 1200 && quiet < 3; i++) {
      const c = srv.ctx.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE meta_state != 0 AND (thumb_state != 0 OR kind NOT IN ('image','video'))")!.c;
      quiet = c >= total && srv.ctx.scanner.idle && !srv.ctx.explore.dups.busy && !srv.ctx.events.genPending ? quiet + 1 : 0;
      await new Promise((res) => setTimeout(res, 100));
    }
  }, 180_000);

  afterAll(async () => {
    await srv?.close();
  });

  it('reads time zone, GPS, place and camera', async () => {
    const s = await byName('selfie.jpg');
    expect(s.localAt).toBe(L('2024-07-01T23:30:00'));
    expect(s.takenAt).toBe(Date.parse('2024-07-01T20:30:00Z'));
    expect(s.flags).toBe(2); // selfie
    const d = await get<MediaDetails>(`/api/media/${s.id}`);
    expect(d.tzOffset).toBe(180);
    expect(d.gps).toEqual({ lat: expect.closeTo(KADIKOY[0], 4), lon: expect.closeTo(KADIKOY[1], 4) });
    expect(d.place).toEqual({ city: 'Kadıköy', admin1: 'İstanbul', cc: 'TR', key: 'TR-34' });
    expect(d.camera).toEqual({ make: 'Samsung', model: 'SM-G991B', lens: 'Samsung Galaxy S21 front camera 2.2mm f/2.2' });

    const n = await get<MediaDetails>(`/api/media/${(await byName('nikon.jpg')).id}`);
    expect(n.tzOffset).toBe(180); // from GPS time
    expect(n.takenAt).toBe(Date.parse('2024-07-03T07:00:00Z'));
    expect(n.camera).toMatchObject({ make: 'Nikon', model: 'D750' });
    expect(n.place).toMatchObject({ admin1: 'Antalya', cc: 'TR' });
  });

  it('classifies screenshots, panoramas, animations; videos read location + maker tags', async () => {
    expect((await byName('Screenshot_20240812-101500.png')).flags).toBe(1);
    expect((await byName('pano.jpg')).flags).toBe(4);
    expect((await byName('anim.gif')).flags).toBe(8);
    const v = await byName('yilbasi.mp4');
    expect(v.takenAt).toBe(Date.parse('2025-12-31T21:30:00Z'));
    expect(v.localAt).toBe(L('2026-01-01T00:30:00')); // home zone Istanbul
    const d = await get<MediaDetails>(`/api/media/${v.id}`);
    expect(d.place).toMatchObject({ cc: 'TR', admin1: 'İstanbul' });
    expect(d.camera).toMatchObject({ make: 'Google', model: 'Pixel 7' });
    expect(d.tzOffset).toBeNull();
    // mtime fallback in the home zone
    const p = await byName('plain.jpg');
    expect(p.takenAt).toBe(Date.parse('2023-03-03T12:00:00Z'));
    expect(p.localAt).toBe(L('2023-03-03T15:00:00'));
  });

  it('filters by place, camera, flag, date and free text', async () => {
    const total = async (qs: string) => (await get<BrowseResult>(`/api/browse?${qs}&limit=1`)).total;
    const ist = await total('place=TR-34');
    expect(ist).toBe(srv.ctx.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE cc = 'TR' AND admin1 = '34'")!.c);
    expect(ist).toBeGreaterThan(5);
    expect(await total('place=TR')).toBe(srv.ctx.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE cc = 'TR'")!.c);
    expect(await total('camera=nikon%7Cd750')).toBe(1);
    expect(await total('camera=nope%7Cx')).toBe(0);
    expect(await total('flag=selfie')).toBe(1);
    expect(await total('flag=screenshot')).toBe(1);
    expect(await total('flag=panorama')).toBe(1);
    expect(await total('flag=animated')).toBe(1);
    expect(await total('flag=video')).toBe(1);
    expect(await total(`from=${L('2024-07-01T00:00:00')}&to=${L('2024-07-05T00:00:00')}`)).toBe(3);
    // the 23:30 selfie belongs to July 1st (local), not the 2nd
    expect(await total(`from=${L('2024-07-02T00:00:00')}&to=${L('2024-07-03T00:00:00')}`)).toBe(0);
    for (const q of ['ISTANBUL', 'istanbul', 'İstanbul', 'Türkiye']) expect(await total(`q=${encodeURIComponent(q)}`), q).toBe(ist + (q === 'Türkiye' ? (await total('place=TR')) - ist : 0));
    expect(await total(`q=${encodeURIComponent('kadıköy selfie')}`)).toBe(1);
    expect(await total(`q=${encodeURIComponent('antalya 2024')}`)).toBe(2 + srv.ctx.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE admin1 = '07' AND name LIKE 'r%' AND taken_local >= ? AND taken_local < ?", L('2024-01-01T00:00:00'), L('2025-01-01T00:00:00'))!.c);
    expect(await total('q=nikon')).toBe(1);
    expect(await total(`q=${encodeURIComponent('ekran görüntüsü')}`)).toBe(1);
    expect(await total('q=temmuz')).toBeGreaterThanOrEqual(3);
    const s = await get<{ items: MediaItem[]; total: number }>(`/api/search?q=${encodeURIComponent('Kadıköy')}`);
    expect(s.total).toBeGreaterThan(1);
  });

  it('skeleton equals the concatenation of browse pages (random filters/sorts)', async () => {
    // a linked phone's folder and a temp session with look-alike rows: never part of the "everything" scope
    const db = srv.ctx.db;
    db.run("INSERT INTO devices (id, name, token_hash, paired, created_at) VALUES ('skeleton-dev-1', 'P', 'x', 1, 0)");
    const devFolder = Number(db.run("INSERT INTO folders (library_id, path, name, device_id, device_key) VALUES (NULL, 'device://skeleton-dev-1/k', 'Cam', 'skeleton-dev-1', 'k')").lastInsertRowid);
    const tempFolder = Number(db.run("INSERT INTO folders (library_id, path, name, temp, expires_at) VALUES (NULL, ?, 'T', 1, ?)", path.join(tmp, 'nowhere'), Date.now() + 3600_000).lastInsertRowid);
    for (const [fid, n] of [[devFolder, 'r1.jpg'], [devFolder, 'r10.jpg'], [tempFolder, 'r1.jpg']] as const) {
      db.run(
        "INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, taken_local, meta_state, meta_v, thumb_state, width, height, cc, admin1) VALUES (?, '', ?, 'jpg', 'image', 1, 1, ?, ?, 1, 99, 1, 10, 10, 'TR', '34')",
        fid,
        n,
        L('2023-05-05T10:00:00'),
        L('2023-05-05T10:00:00'),
      );
    }
    srv.ctx.events.bump();
    let seed = 11;
    const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    const pick = <T>(a: T[]) => a[Math.floor(rnd() * a.length)];
    for (let run = 0; run < 25; run++) {
      const params = new URLSearchParams();
      params.set('sort', pick(['takenAt', 'name', 'mtime', 'size', 'kind']));
      params.set('order', pick(['asc', 'desc']));
      const loc = pick(['all', 'folder', 'dir', 'lib']);
      if (loc === 'folder') params.set('folderId', '1'), params.set('recursive', '1');
      if (loc === 'dir') params.set('folderId', '1'), params.set('dir', pick(['Rastgele', 'Rastgele/a', 'Tatil'])), params.set('recursive', pick(['1', '0']));
      if (loc === 'lib') params.set('libraryId', '1');
      const f = pick(['none', 'place', 'camera', 'flag', 'date', 'q', 'kind']);
      if (f === 'place') params.set('place', pick(['TR-34', 'TR-07', 'TR', 'DE-16']));
      if (f === 'camera') params.set('camera', pick(['apple|iphone 13', 'samsung|sm-g991b']));
      if (f === 'flag') params.set('flag', pick(['video', 'selfie', 'panorama']));
      if (f === 'date') params.set('from', String(L('2022-06-01T00:00:00'))), params.set('to', String(L('2024-01-01T00:00:00')));
      if (f === 'q') params.set('q', pick(['istanbul', 'r1', '2023', 'iphone']));
      if (f === 'kind') params.set('kind', pick(['image', 'video', 'favorites']));
      const sk = await get<BrowseSkeleton>(`/api/browse/skeleton?${params}`);
      const aspects = Buffer.from(sk.aspects, 'base64');
      expect(aspects.length).toBe(sk.total);
      const limit = 1 + Math.floor(rnd() * 7);
      const all: MediaItem[] = [];
      let gen = '';
      for (let off = 0; ; off += limit) {
        const p = await get<BrowseResult>(`/api/browse?${params}&offset=${off}&limit=${limit}`);
        expect(p.total).toBe(sk.total);
        gen = p.gen!;
        all.push(...p.items);
        if (off + limit >= p.total) break;
      }
      expect(gen, params.toString()).toBe(sk.gen);
      expect(all.length, params.toString()).toBe(sk.total);
      if (loc !== 'dir' && loc !== 'folder') expect(all.every((it) => it.folderId === '1'), params.toString()).toBe(true);
      all.forEach((it, i) => {
        const a = it.width && it.height ? it.width / it.height : 1;
        expect(Math.abs(Math.log2(byteToAspect(aspects[i]) / a)), `${params} #${i}`).toBeLessThan(0.02);
      });
      const kinds = { image: 0, video: 0, audio: 0 };
      for (const it of all) if (it.kind in kinds) kinds[it.kind as keyof typeof kinds]++;
      expect(sk.kindCounts).toEqual(kinds);
      if (params.get('sort') === 'takenAt') {
        expect(sk.days!.reduce((n, d) => n + d.count, 0)).toBe(sk.total);
        const flat = sk.days!.flatMap((d) => Array.from({ length: d.count }, () => d.key));
        expect(flat).toEqual(all.map((it) => localDayKey(it.localAt)));
      } else expect(sk.days).toBeNull();
    }
    const libR1 = db.get<{ c: number }>("SELECT count(*) c FROM media WHERE folder_id = 1 AND kind IN ('image','video','audio') AND instr(peek_search(name, city, cc, admin1, make, model), 'r1') > 0")!.c;
    expect((await get<BrowseSkeleton>('/api/browse/skeleton?q=r1')).total).toBe(libR1);
    expect((await get<{ total: number }>('/api/search?q=r1')).total).toBe(libR1);
    expect((await get<BrowseSkeleton>(`/api/browse/skeleton?folderId=${devFolder}&q=r1`)).total).toBe(2);
    db.run('DELETE FROM folders WHERE id IN (?, ?)', devFolder, tempFolder);
    db.run("DELETE FROM devices WHERE id = 'skeleton-dev-1'");
    srv.ctx.events.bump();
  });

  it('labels skeleton days with the dominant city', async () => {
    const sk = await get<BrowseSkeleton>(`/api/browse/skeleton?folderId=1&dir=Tatil`);
    expect(sk.days).toEqual([
      { key: '2024-07-04', count: 1, place: 'Muratpaşa' },
      { key: '2024-07-03', count: 1, place: 'Muratpaşa' },
      { key: '2024-07-01', count: 1, place: 'Kadıköy' },
    ]);
  });

  it('explore groups places, types, cameras, years', async () => {
    const ex = await get<ExploreResult>('/api/explore');
    const ist = ex.places.find((p) => p.key === 'TR-34')!;
    expect(ist).toMatchObject({ cc: 'TR', name: 'İstanbul' });
    expect(ist.lat).toBeCloseTo(41, 0);
    expect(ex.places.find((p) => p.key === 'TR-07')).toMatchObject({ name: 'Antalya' });
    expect(ex.types.map((t) => t.flag).sort()).toEqual(['animated', 'panorama', 'screenshot', 'selfie', 'video']);
    expect(ex.cameras.map((c) => c.key)).toEqual(expect.arrayContaining(['nikon|d750', 'samsung|sm-g991b', 'apple|iphone 13', 'google|pixel 7']));
    expect(ex.cameras.find((c) => c.key === 'nikon|d750')).toMatchObject({ make: 'Nikon', model: 'D750', count: 1 });
    expect(ex.years.map((y) => y.year)).toEqual([...ex.years.map((y) => y.year)].sort((a, b) => b - a));
    expect(ex.years.reduce((n, y) => n + y.count, 0)).toBe(srv.ctx.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE kind IN ('image','video')")!.c);
    expect(ex.indexing.done).toBe(ex.indexing.total);
    // the same result set via browse filters
    for (const p of ex.places) expect((await get<BrowseResult>(`/api/browse?place=${p.key}&limit=1`)).total).toBe(p.count);
    for (const c of ex.cameras) expect((await get<BrowseResult>(`/api/browse?camera=${encodeURIComponent(c.key)}&limit=1`)).total).toBe(c.count);
    for (const t of ex.types) expect((await get<BrowseResult>(`/api/browse?flag=${t.flag}&kind=image-video&limit=1`)).total).toBe(t.count);
  });

  it('serves geo points with the browse filters', async () => {
    const pts = await get<{ points: [string, number, number][] }>('/api/geo/points?place=TR-07');
    expect(pts.points.length).toBe((await get<BrowseResult>('/api/browse?place=TR-07&limit=1')).total);
    for (const [id, lat, lon] of pts.points) {
      expect(typeof id).toBe('string');
      expect(lat).toBeCloseTo(ANTALYA[0], 1);
      expect(lon).toBeCloseTo(ANTALYA[1], 1);
    }
  });

  it('serves bounded display renditions for zoom', async () => {
    const pano = await byName('pano.jpg');
    const r = await srv.app.inject(`/api/media/${pano.id}/display?max=2048`);
    expect(r.statusCode).toBe(200);
    expect(r.headers['content-type']).toContain('image/jpeg');
    const m = await sharp(r.rawPayload).metadata();
    expect(Math.max(m.width!, m.height!)).toBe(2048);
    const small = await byName('plain.jpg');
    expect((await srv.app.inject(`/api/media/${small.id}/display?max=4096`)).statusCode).toBe(302);
    const png = await byName('Screenshot_20240812-101500.png');
    expect((await srv.app.inject(`/api/media/${png.id}/display`)).statusCode).toBe(302);
  });

  it('keeps upload mtime and changes gen', async () => {
    const before = await get<BrowseResult>('/api/browse?folderId=1&dir=Yukle&limit=1');
    const mtime = Date.parse('2021-05-06T07:08:09Z');
    const body = fs.readFileSync(path.join(lib, 'plain.jpg'));
    const r = await srv.app.inject({ method: 'PUT', url: `/api/upload?folderId=1&dir=Yukle&path=up.jpg&mtime=${mtime}`, payload: body, headers: { 'content-type': 'application/octet-stream' } });
    expect(r.statusCode).toBe(200);
    expect(fs.statSync(path.join(lib, 'Yukle', 'up.jpg')).mtimeMs).toBe(mtime);
    const after = await get<BrowseResult>('/api/browse?folderId=1&dir=Yukle&limit=1');
    expect(after.total).toBe(before.total + 1);
    expect(after.gen).not.toBe(before.gen);
    expect(after.items[0].takenAt).toBe(mtime);
  });

  it('timeline buckets use the local wall clock', async () => {
    const t = await get<{ buckets: { key: string; count: number }[] }>('/api/timeline');
    expect(t.buckets.find((b) => b.key === '2026-01')?.count).toBe(1); // video at 00:30 local on Jan 1
  });

  it('tags media events with a reason', () => {
    const media = events.filter((e): e is Extract<ServerEvent, { type: 'media' }> => e.type === 'media');
    expect(media.length).toBeGreaterThan(0);
    for (const e of media) expect(['add', 'remove', 'change', 'meta', 'thumb']).toContain(e.reason);
    expect(media.some((e) => e.reason === 'add')).toBe(true);
  });

  it('enriches old rows in the background without per-row media events', async () => {
    const db = srv.ctx.db;
    const snapshot = db.all('SELECT id, cc, admin1, city, make, model, flags, taken_local, tz_off FROM media ORDER BY id');
    db.run("UPDATE media SET meta_v = 0, cc = NULL, admin1 = NULL, city = NULL, make = NULL, model = NULL, flags = 0, lat = NULL, lon = NULL WHERE kind IN ('image','video')");
    const ex0 = await get<ExploreResult>('/api/explore');
    expect(ex0.indexing.done).toBeLessThan(ex0.indexing.total);
    for (let i = 0; i < 100 && srv.ctx.scanner.busy; i++) await new Promise((res) => setTimeout(res, 50));
    srv.ctx.events.close(); // drop throttled events still pending from earlier tests
    const mark = events.length;
    srv.ctx.scanner.kickEnrich();
    for (let i = 0; i < 200 && (srv.ctx.scanner.enriching || db.get<{ c: number }>('SELECT count(*) c FROM media WHERE meta_v < ?', META_V)!.c); i++) {
      await new Promise((res) => setTimeout(res, 50));
    }
    expect(db.all('SELECT id, cc, admin1, city, make, model, flags, taken_local, tz_off FROM media ORDER BY id')).toEqual(snapshot);
    const ex = await get<ExploreResult>('/api/explore');
    expect(ex.indexing).toEqual({ done: ex0.indexing.total, total: ex0.indexing.total });
    expect(ex.places.length).toBeGreaterThan(0);
    // dates did not change → no layout events at all
    expect(events.slice(mark).filter((e) => e.type === 'media')).toHaveLength(0);
    for (let i = 0; i < 150 && !events.slice(mark).some((e) => e.type === 'explore'); i++) await new Promise((res) => setTimeout(res, 100));
    expect(events.slice(mark).filter((e) => e.type === 'explore').length).toBe(1);
  }, 30_000);

  it('re-derives local times when the home zone changes', async () => {
    const v0 = await byName('yilbasi.mp4');
    const s0 = await byName('selfie.jpg');
    const r = await srv.app.inject({ method: 'PATCH', url: '/api/settings', payload: { homeTimeZone: 'America/New_York' } });
    expect(r.json().homeTimeZone).toBe('America/New_York');
    const v = await byName('yilbasi.mp4');
    expect(v.takenAt).toBe(v0.takenAt);
    expect(v.localAt).toBe(instantToLocal(v0.takenAt, 'America/New_York'));
    expect(v.localAt).toBe(v0.localAt - 8 * 60 * MIN);
    // a photo with a known offset never moves
    expect(await byName('selfie.jpg')).toMatchObject({ localAt: s0.localAt, takenAt: s0.takenAt });
    expect((await srv.app.inject({ method: 'PATCH', url: '/api/settings', payload: { homeTimeZone: 'Mars/Olympus' } })).statusCode).toBe(400);
  });
});
