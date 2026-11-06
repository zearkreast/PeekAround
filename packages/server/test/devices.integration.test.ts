import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import WebSocket from 'ws';
import { instantToLocal, type DeviceInfo, type Job, type ServerEvent } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import { FIRST_BYTE_MS } from '../src/devices/hub';
import { FakePhone, sha, virtualBytes } from './helpers/fake-phone';

const FX = path.resolve(__dirname, '../../shared/fixtures/link');
const fixture = (n: string) => JSON.parse(fs.readFileSync(path.join(FX, n), 'utf8')) as any[];

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function until<T>(fn: () => Promise<T | null | undefined | false> | T | null | undefined | false, ms = 10_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await fn();
    if (v) return v as T;
    if (Date.now() > end) throw new Error('condition not met in time');
    await sleep(40);
  }
}

function freePort(): Promise<number> {
  return new Promise((resolve) => {
    const s = net.createServer();
    s.listen(0, '127.0.0.1', () => {
      const p = (s.address() as net.AddressInfo).port;
      s.close(() => resolve(p));
    });
  });
}

describe('linked phones (device tunnel)', () => {
  let srv: PeekServer;
  let base: string;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-dev-'));
  const lib = path.join(tmp, 'lib');
  const phoneRoot = path.join(tmp, 'phone', 'Camera');
  const photo = path.join(lib, 'photo.jpg');
  const vid = path.join(phoneRoot, 'Trip', 'VID_2.mp4');
  const vidBytes = virtualBytes(0, 4096);
  const pairing = fixture('pairing.json');
  const hello = pairing[0].frame;
  const PHONE_ID: string = hello.id;
  let phone: FakePhone;
  let devFolder: string;
  let libFolder: string;
  let imgId: string;
  let vidId: string;
  const events: ServerEvent[] = [];

  const api = (p: string, init: RequestInit = {}) => fetch(`${base}${p}`, init);
  const j = (r: Response): Promise<any> => r.json();
  const j0 = (method: string, body: unknown): RequestInit => ({ method, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const json = (method: string, body: unknown, headers: Record<string, string> = {}): RequestInit => ({
    method,
    headers: { 'Content-Type': 'application/json', ...headers },
    body: JSON.stringify(body),
  });

  beforeAll(async () => {
    fs.mkdirSync(path.join(phoneRoot, 'Trip'), { recursive: true });
    fs.mkdirSync(path.join(phoneRoot, 'Empty'), { recursive: true });
    fs.mkdirSync(lib, { recursive: true });
    await sharp({ create: { width: 800, height: 600, channels: 3, background: '#3b82f6' } }).jpeg().toFile(path.join(phoneRoot, 'IMG_1.jpg'));
    fs.writeFileSync(vid, vidBytes);
    fs.utimesSync(vid, new Date(), new Date(1700000500000));
    await sharp({ create: { width: 320, height: 200, channels: 3, background: '#f97316' } }).jpeg().toFile(photo);
    fs.utimesSync(photo, new Date(), new Date(1690000000000));
    srv = await createServer({ mode: 'desktop', dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false });
    base = srv.url;
    srv.ctx.events.on((e) => events.push(e));
    srv.ctx.events.intervals = { add: 10, remove: 10, change: 10, meta: 10, thumb: 10 };
    libFolder = (await j(await api('/api/libraries'))).libraries[0].folders[0].id;
  });

  afterAll(async () => {
    await srv?.close();
  });

  it('pairs a new phone following the golden pairing transcript', async () => {
    phone = new FakePhone(base, PHONE_ID, [{ key: 'k1', name: 'Kamera', root: phoneRoot }]);
    phone.listBody = fixture('list-and-thumb.json')[1].body;
    const first = await phone.connect(hello);
    expect(Object.keys(first).sort()).toEqual(Object.keys(pairing[1].frame).sort());
    expect(first.t).toBe('pending');
    expect(first.code).toMatch(/^\d{4}$/);
    const pair = events.find((e) => e.type === 'pair') as Extract<ServerEvent, { type: 'pair' }>;
    expect(pair.device).toMatchObject({ id: PHONE_ID, name: 'Test Phone', paired: false, code: first.code });
    const pending = (await j(await api('/api/devices'))) as DeviceInfo[];
    expect(pending).toEqual([expect.objectContaining({ id: PHONE_ID, paired: false, online: true, code: first.code })]);

    // a device can never approve itself
    expect((await api(`/api/devices/${PHONE_ID}/approve`, json('POST', {}, { 'X-Peek-Device': PHONE_ID }))).status).toBe(403);
    expect((await api(`/api/devices/${PHONE_ID}/approve`, json('POST', {}))).status).toBe(200);
    const paired = await phone.waitFrame((f) => f.t === 'paired');
    expect(Object.keys(paired)).toEqual(Object.keys(pairing[2].frame));
    expect(paired.token).toMatch(/^[A-Za-z0-9_-]{43}$/);
    const row = srv.ctx.db.get<{ token_hash: string; paired: number }>('SELECT token_hash, paired FROM devices WHERE id = ?', PHONE_ID)!;
    expect(row.paired).toBe(1);
    expect(row.token_hash).toBe(crypto.createHash('sha256').update(paired.token).digest('hex'));

    // the server lists the shared folder right away (fixture NDJSON body)
    const listReq = await phone.waitFrame((f) => f.t === 'req' && f.op === 'list').catch(() => null);
    if (listReq) expect(listReq.args).toEqual(fixture('list-and-thumb.json')[0].frame.args);
    const devs = await until(async () => {
      const d = (await j(await api('/api/devices'))) as DeviceInfo[];
      return d[0]?.folders[0]?.mediaCount === 2 ? d : null;
    });
    expect(devs[0]).toMatchObject({ id: PHONE_ID, paired: true, online: true, caps: { read: true, receive: true } });
    devFolder = devs[0].folders[0].id;
    expect(devs[0].folders[0]).toMatchObject({ name: 'Kamera', libraryId: null, temp: false, path: `device://${PHONE_ID}/k1`, device: { id: PHONE_ID, name: 'Test Phone', online: true } });
  });

  it('shows the phone folder as a regular folder (rows, sub folders, separate from libraries)', async () => {
    const libs = await j(await api('/api/libraries'));
    expect(libs.libraries.flatMap((l: any) => l.folders.map((f: any) => f.id))).not.toContain(devFolder);
    expect(libs.temp).toEqual([]);
    expect(libs.devices[0].folders[0].id).toBe(devFolder);

    const root = await j(await api(`/api/browse?folderId=${devFolder}`));
    expect(root.items.map((i: any) => i.name)).toEqual(['IMG_1.jpg']);
    expect(root.folders.map((f: any) => f.name)).toEqual(['Empty', 'Trip']);
    expect(root.folders.find((f: any) => f.name === 'Trip').mediaCount).toBe(1);
    expect(root.target).toBeNull();
    const img = root.items[0];
    imgId = img.id;
    expect(img).toMatchObject({ width: 4000, height: 3000, size: 2048, mtime: 1700000000000, takenAt: 1699999999000, playback: 'direct' });
    expect(img.localAt).toBe(instantToLocal(1699999999000, srv.ctx.scanner.homeTimeZone));

    const trip = await j(await api(`/api/browse?folderId=${devFolder}&dir=Trip`));
    expect(trip.items).toHaveLength(1);
    expect(trip.items[0]).toMatchObject({ name: 'VID_2.mp4', kind: 'video', duration: 12.5, width: 1920, height: 1080 });
    vidId = trip.items[0].id;
    const empty = await j(await api(`/api/browse?folderId=${devFolder}&dir=Empty`));
    expect(empty.items).toEqual([]);

    // home / timeline only shows libraries
    const home = await j(await api('/api/browse?recursive=1'));
    expect(home.items.map((i: any) => i.id)).not.toContain(imgId);
  });

  it('fetches thumbnails through the tunnel and caches them', async () => {
    const t = await api(`/api/media/${imgId}/thumb?s=320`);
    expect(t.status).toBe(200);
    expect(t.headers.get('content-type')).toContain('image/webp');
    const req = await phone.waitFrame((f) => f.t === 'req' && f.op === 'thumb' && f.args.p === 'IMG_1.jpg');
    expect(req.rid).toMatch(/^[A-Za-z0-9_-]{22}$/);
    expect(req.args).toMatchObject({ key: 'k1', p: 'IMG_1.jpg' });
    expect([320, 640, 1600]).toContain(req.args.s);
    // the background loop also fills thumbhashes
    await until(async () => (await j(await api(`/api/browse?folderId=${devFolder}`))).items[0].thumbhash);
  });

  it('serves byte-exact ranges like the golden file-range transcript', async () => {
    const fx = fixture('file-range.json');
    const r = await api(`/api/media/${vidId}/file`, { headers: { Range: 'bytes=100-199' } });
    expect(r.status).toBe(206);
    const req = await phone.waitFrame((f) => f.t === 'req' && f.op === 'file' && f.args.range === 'bytes=100-199');
    expect(req.args).toEqual(fx[0].frame.args);
    expect(r.headers.get('content-range')).toBe(fx[1].headers['X-Peek-Content-Range']);
    expect(r.headers.get('content-length')).toBe('100');
    expect(r.headers.get('accept-ranges')).toBe('bytes');
    expect(r.headers.get('content-type')).toBe('video/mp4');
    expect(sha(Buffer.from(await r.arrayBuffer()))).toBe(sha(vidBytes.subarray(100, 200)));
    expect((await phone.puts.get(req.rid))!.status).toBe(204);

    const bad = await api(`/api/media/${vidId}/file`, { headers: { Range: 'bytes=5000-' } });
    expect(bad.status).toBe(416);
    expect(bad.headers.get('content-range')).toBe('bytes */4096');

    const suffix = await api(`/api/media/${vidId}/file`, { headers: { Range: 'bytes=-10' } });
    expect(suffix.status).toBe(206);
    expect(Buffer.from(await suffix.arrayBuffer()).equals(vidBytes.subarray(4086))).toBe(true);

    const full = await api(`/api/media/${vidId}/file`);
    expect(full.status).toBe(200);
    expect(full.headers.get('content-length')).toBe('4096');
    expect(sha(Buffer.from(await full.arrayBuffer()))).toBe(sha(vidBytes));

    // device error frames map to HTTP errors
    phone.intercept = (f) => {
      if (f.op !== 'file') return false;
      phone.intercept = null;
      phone.send({ t: 'err', rid: f.rid, status: 404, message: 'not found' });
      return true;
    };
    expect((await api(`/api/media/${vidId}/file`)).status).toBe(404);

    // HEAD answers from the index
    const head = await api(`/api/media/${vidId}/file`, { method: 'HEAD' });
    expect(head.status).toBe(200);
    expect(head.headers.get('content-length')).toBe('4096');
  });

  it('details come from the phone and enrich the row', async () => {
    const d = await j(await api(`/api/media/${imgId}`));
    expect(d.camera).toMatchObject({ make: 'Google', model: 'Pixel 8' });
    expect(d.gps).toEqual({ lat: 40.99, lon: 29.03 });
    expect(d.exif).toMatchObject({ Make: 'Google', FNumber: 1.7 });
    expect(d.path).toBe('Test Phone/Kamera/IMG_1.jpg');
  });

  it('renditions (display, filmstrip) and live transcoding come through the tunnel', async () => {
    const disp = await api(`/api/media/${imgId}/display?max=2048`);
    expect(disp.status).toBe(200);
    expect((await phone.waitFrame((f) => f.t === 'req' && f.op === 'display')).args).toEqual({ key: 'k1', p: 'IMG_1.jpg', max: 2048 });
    const strip = await api(`/api/media/${vidId}/filmstrip?n=4&h=72`);
    expect(strip.status).toBe(200);
    expect((await phone.waitFrame((f) => f.t === 'req' && f.op === 'strip')).args).toEqual({ key: 'k1', p: 'Trip/VID_2.mp4', n: 4, h: 72 });
    const prev = await api(`/api/media/${vidId}/preview`, { redirect: 'manual' });
    expect(prev.status).toBe(302);
    expect(prev.headers.get('location')).toBe(`/api/media/${vidId}/file`);
    if (!srv.ctx.ffmpeg.available) return;
    // a real (moov-at-end) video on the phone, transcoded by ffmpeg reading our ranged /file endpoint
    const real = path.join(phoneRoot, 'Trip', 'real.mkv');
    await srv.ctx.ffmpeg.run(['-y', '-f', 'lavfi', '-i', 'testsrc=size=160x120:rate=10', '-t', '2', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', real], { timeoutMs: 60_000 });
    expect((await api(`/api/folders/${devFolder}/rescan`, j0('POST', {}))).status).toBe(200);
    const item = await until(async () => (await j(await api(`/api/browse?folderId=${devFolder}&dir=Trip`))).items.find((i: any) => i.name === 'real.mkv'));
    expect(item.playback).toBe('transcode');
    const st = await api(`/api/media/${item.id}/stream`);
    expect(st.status).toBe(200);
    expect(st.headers.get('content-type')).toBe('video/mp4');
    const body = Buffer.from(await st.arrayBuffer());
    expect(body.length).toBeGreaterThan(1000);
    expect(body.subarray(4, 8).toString()).toBe('ftyp');
  });

  async function addVirtual(rel: string, size: number) {
    phone.virtual.set(`k1/${rel}`, { size, mtime: 1700001000000 });
    expect((await api(`/api/folders/${devFolder}/rescan`, json('POST', {}))).status).toBe(200);
    const dir = path.posix.dirname(rel);
    const item = await until(async () => {
      const b = await j(await api(`/api/browse?folderId=${devFolder}&dir=${encodeURIComponent(dir)}`));
      return b.items.find((i: any) => i.name === path.posix.basename(rel));
    });
    return item.id as string;
  }

  it('consumer abort → cancel frame and the reply PUT gets 499', async () => {
    const id = await addVirtual('Big/abort.mp4', 32 * 1024 * 1024);
    const got = new Promise<number>((resolve) => {
      const req = http.get(`${base}/api/media/${id}/file`, (res) => {
        res.once('data', (c: Buffer) => {
          resolve(c.length);
          req.destroy();
        });
      });
      req.on('error', () => {});
    });
    expect(await got).toBeGreaterThan(0);
    const fileReq = await phone.waitFrame((f) => f.t === 'req' && f.op === 'file' && f.args.p === 'Big/abort.mp4');
    const cancel = await phone.waitFrame((f) => f.t === 'cancel' && f.rid === fileReq.rid);
    expect(Object.keys(cancel)).toEqual(Object.keys(fixture('file-range.json')[4].frame));
    const put = await phone.puts.get(fileReq.rid)!;
    expect(put.status).toBe(499);
    expect(put.sent).toBeLessThan(32 * 1024 * 1024);
    await until(() => srv.ctx.devices.stats(PHONE_ID).inflight === 0);
  });

  it('slow consumer: backpressure keeps memory bounded (50 MB)', async () => {
    const size = 50 * 1024 * 1024;
    const id = await addVirtual('Big/large.mp4', size);
    phone.written = 0;
    let read = 0;
    let maxGap = 0;
    const rss0 = process.memoryUsage().rss;
    let maxRss = rss0;
    const hash = crypto.createHash('sha256');
    await new Promise<void>((resolve, reject) => {
      http
        .get(`${base}/api/media/${id}/file`, (res) => {
          expect(res.statusCode).toBe(200);
          res.pause();
          const timer = setInterval(() => {
            // throttled reader: ~256 KB per tick
            let budget = 256 * 1024;
            let c: Buffer | null;
            while (budget > 0 && (c = res.read()) !== null) {
              read += c.length;
              budget -= c.length;
              hash.update(c);
            }
            maxGap = Math.max(maxGap, phone.written - read);
            maxRss = Math.max(maxRss, process.memoryUsage().rss);
          }, 5);
          res.on('end', () => {
            clearInterval(timer);
            resolve();
          });
          res.on('error', reject);
        })
        .on('error', reject);
    });
    expect(read).toBe(size);
    expect(hash.digest('hex')).toBe(sha(virtualBytes(0, size)));
    // at most socket + stream buffers in flight between phone and browser — never the file
    expect(maxGap).toBeLessThan(24 * 1024 * 1024);
    expect(maxRss - rss0).toBeLessThan(48 * 1024 * 1024);
  }, 60_000);

  it('first byte timeout → 504 and a cancel frame', async () => {
    const prev = FIRST_BYTE_MS.file;
    FIRST_BYTE_MS.file = 300;
    phone.ignore.add('file');
    phone.frames.length = 0;
    try {
      const r = await api(`/api/media/${vidId}/file`);
      expect(r.status).toBe(504);
      const req = await phone.waitFrame((f) => f.t === 'req' && f.op === 'file' && f.args.p === 'Trip/VID_2.mp4');
      await phone.waitFrame((f) => f.t === 'cancel' && f.rid === req.rid);
      // a late reply for that rid is refused
      expect((await phone.put(req.rid, {}, Buffer.from('late'))).status).toBe(410);
    } finally {
      FIRST_BYTE_MS.file = prev;
      phone.ignore.delete('file');
    }
  });

  it('requires the device token on phone endpoints', async () => {
    const rid = 'A'.repeat(22);
    expect((await phone.put(rid, {}, Buffer.from('x'), {})).status).toBe(401);
    expect((await phone.put(rid, {}, Buffer.from('x'), { Authorization: `Bearer ${'x'.repeat(43)}`, 'X-Peek-Device': PHONE_ID })).status).toBe(401);
    expect((await phone.put(rid, {}, Buffer.from('x'))).status).toBe(410);
    expect((await api('/api/devices/pull/whatever')).status).toBe(401);
    expect((await api('/api/devices/pull/whatever', { headers: { Authorization: `Bearer ${phone.token}`, 'X-Peek-Device': PHONE_ID } })).status).toBe(404);
  });

  it('copies device → PC as a transfer job (bytes + mtime preserved)', async () => {
    expect((await api('/api/fs/move', json('POST', { refs: [{ folderId: devFolder, rel: 'Trip/VID_2.mp4' }], to: { folderId: libFolder, dir: 'FromPhone' } }))).status).toBe(403);
    const r = await api('/api/fs/move', json('POST', { refs: [{ folderId: devFolder, rel: 'Trip/VID_2.mp4' }], to: { folderId: libFolder, dir: 'FromPhone' }, copy: true }));
    expect(r.status).toBe(200);
    const { moved, job } = (await r.json()) as { moved: string[]; job: Job };
    expect(moved).toEqual([]);
    expect(job).toMatchObject({ type: 'transfer', files: { done: 0, total: 1 }, bytes: { done: 0, total: 4096 } });
    const fin = (await srv.ctx.jobs.wait(job.id))!;
    expect(fin.status).toBe('done');
    expect(fin.bytes).toEqual({ done: 4096, total: 4096 });
    expect(fin.result).toEqual({ folderId: libFolder, dir: 'FromPhone', name: 'VID_2.mp4' });
    const dest = path.join(lib, 'FromPhone', 'VID_2.mp4');
    expect(sha(fs.readFileSync(dest))).toBe(sha(vidBytes));
    expect(Math.abs(fs.statSync(dest).mtimeMs - 1700000500000)).toBeLessThan(2);
    expect(fs.readdirSync(path.join(lib, 'FromPhone')).filter((n) => n.endsWith('.part'))).toEqual([]);

    // a whole directory keeps its tree
    const r2 = await j(await api('/api/fs/move', json('POST', { refs: [{ folderId: devFolder, rel: 'Trip' }], to: { folderId: libFolder, dir: 'FromPhone' }, copy: true })));
    expect((await srv.ctx.jobs.wait(r2.job.id))!.status).toBe('done');
    expect(sha(fs.readFileSync(path.join(lib, 'FromPhone', 'Trip', 'VID_2.mp4')))).toBe(sha(vidBytes));
    const indexed = await j(await api(`/api/browse?folderId=${libFolder}&dir=FromPhone%2FTrip`));
    expect(indexed.items.map((i: any) => i.name)).toContain('VID_2.mp4');
  });

  it('copies PC → device with the receive flow (one-time pull URL)', async () => {
    let pullUrl = '';
    phone.intercept = (f) => {
      if (f.op === 'receive') pullUrl = f.args.url;
      return false;
    };
    const r = await j(await api('/api/fs/move', json('POST', { refs: [{ folderId: libFolder, rel: 'photo.jpg' }], to: { folderId: devFolder, dir: 'From PC' }, copy: true })));
    expect(r.job.type).toBe('transfer');
    const fin = (await srv.ctx.jobs.wait(r.job.id))!;
    phone.intercept = null;
    expect(fin.status).toBe('done');
    expect(fin.result).toEqual({ folderId: devFolder, dir: 'From PC', name: 'photo.jpg' });
    const recv = await phone.waitFrame((f) => f.t === 'req' && f.op === 'receive');
    expect(Object.keys(recv.args).sort()).toEqual(Object.keys(fixture('receive.json')[0].frame.args).sort());
    expect(recv.args).toMatchObject({ key: 'k1', dir: 'From PC', name: 'photo.jpg', size: fs.statSync(photo).size, mtime: 1690000000000 });
    const fxGet = fixture('receive.json')[1].response;
    for (const h of Object.keys(fxGet.headers)) expect(phone.lastPullHeaders[h.toLowerCase()], h).toBeDefined();
    expect(phone.lastPullHeaders['x-peek-mtime']).toBe('1690000000000');
    expect(phone.lastPullHeaders['last-modified']).toBe(fxGet.headers['Last-Modified']);
    const landed = path.join(phoneRoot, 'From PC', 'photo.jpg');
    expect(sha(fs.readFileSync(landed))).toBe(sha(fs.readFileSync(photo)));
    expect(Math.abs(fs.statSync(landed).mtimeMs - 1690000000000)).toBeLessThan(2);
    // one-time: the URL is gone
    expect(pullUrl).toMatch(/^\/api\/devices\/pull\/[A-Za-z0-9_-]+$/);
    expect((await api(pullUrl, { headers: { Authorization: `Bearer ${phone.token}`, 'X-Peek-Device': PHONE_ID } })).status).toBe(404);
    // the server re-lists the folder afterwards
    await until(async () => (await j(await api(`/api/browse?folderId=${devFolder}&dir=From%20PC`))).items.length === 1);
  });

  it('protects device folders from writes', async () => {
    const post = (p: string, b: unknown) => api(p, json('POST', b));
    expect((await post('/api/fs/rename', { folderId: devFolder, rel: 'IMG_1.jpg', name: 'x.jpg' })).status).toBe(403);
    expect((await post('/api/fs/delete', { refs: [{ folderId: devFolder, rel: 'IMG_1.jpg' }] })).status).toBe(403);
    expect((await post('/api/fs/mkdir', { folderId: devFolder, dir: '', name: 'New' })).status).toBe(403);
    expect((await post('/api/fs/move', { refs: [{ folderId: libFolder, rel: 'photo.jpg' }], to: { folderId: devFolder, dir: '' } })).status).toBe(403);
    expect((await api(`/api/upload?folderId=${devFolder}&path=a.jpg`, { method: 'PUT', body: Buffer.from('x'), headers: { 'Content-Type': 'application/octet-stream' } })).status).toBe(403);
    expect((await api(`/api/folders/${devFolder}`, json('PATCH', { name: 'x' }))).status).toBe(403);
    expect((await api(`/api/folders/${devFolder}`, { method: 'DELETE' })).status).toBe(403);
    expect((await api(`/api/edits/save?sourceId=${imgId}`, { method: 'POST', body: fs.readFileSync(photo), headers: { 'Content-Type': 'application/octet-stream' } })).status).toBe(403);
    const ve = await post('/api/jobs/video-edit', { sourceId: vidId, output: { format: 'mp4', height: null, quality: 'low' } });
    expect([403, 501]).toContain(ve.status);
    expect(fs.existsSync(path.join(process.cwd(), 'device:'))).toBe(false);
  });

  it('a phone never sees its own folders', async () => {
    const own = { 'X-Peek-Device': PHONE_ID };
    const libs = await j(await api('/api/libraries', { headers: own }));
    expect(libs.devices.map((d: DeviceInfo) => d.id)).not.toContain(PHONE_ID);
    expect(((await j(await api('/api/devices', { headers: own }))) as DeviceInfo[]).map((d) => d.id)).not.toContain(PHONE_ID);
    expect((await api(`/api/browse?folderId=${devFolder}`, { headers: own })).status).toBe(404);
    expect((await api(`/api/media/${imgId}/thumb?s=320`, { headers: own })).status).toBe(404);
    expect((await api(`/api/browse?folderId=${devFolder}`)).status).toBe(200);
  });

  it('rejects a second phone (4403)', async () => {
    const p2 = new FakePhone(base, 'aaaaaaaa-0000-4000-8000-000000000002', [{ key: 'x', name: 'X', root: phoneRoot }], { name: 'Other' });
    expect((await p2.connect()).t).toBe('pending');
    expect((await api(`/api/devices/aaaaaaaa-0000-4000-8000-000000000002/reject`, json('POST', {}))).status).toBe(200);
    await p2.waitFrame((f) => f.t === 'rejected');
    expect(await p2.closed()).toBe(4403);
    expect(srv.ctx.db.get('SELECT * FROM devices WHERE id = ?', 'aaaaaaaa-0000-4000-8000-000000000002')).toBeUndefined();
  });

  it('a reconnect with the token gets welcome and replaces the older socket (4409)', async () => {
    const again = new FakePhone(base, PHONE_ID, phone.folders, { token: phone.token });
    again.virtual = phone.virtual;
    const first = await again.connect();
    expect(first).toMatchObject({ t: 'welcome', paired: true });
    expect(await phone.closed()).toBe(4409);
    phone = again;
    // a wrong token never gets in: it is treated like a new phone (pending)
    const impostor = new FakePhone(base, PHONE_ID, phone.folders, { token: 'y'.repeat(43) });
    expect((await impostor.connect()).t).toBe('pending');
    expect(srv.ctx.devices.isOnline(PHONE_ID)).toBe(true);
    impostor.close();
    await until(async () => ((await j(await api('/api/devices'))) as DeviceInfo[]).every((d) => d.paired));
  });

  it('offline: cached thumbnails still served, file requests 503', async () => {
    phone.close();
    await until(async () => ((await j(await api('/api/devices'))) as DeviceInfo[])[0].online === false);
    const lib2 = await j(await api('/api/libraries'));
    expect(lib2.devices[0].folders[0].device.online).toBe(false);
    expect((await api(`/api/media/${imgId}/thumb?s=320`)).status).toBe(200);
    const f = await api(`/api/media/${vidId}/file`);
    expect(f.status).toBe(503);
    expect((await j(f)).error).toBe('device offline');
    expect((await api(`/api/media/${imgId}/thumb?s=1600`)).status).toBe(404);
    // rows stay browsable
    expect((await j(await api(`/api/browse?folderId=${devFolder}`))).items.length).toBe(1);
  });

  it('works through the desktop LAN server; stopping sharing closes every LAN socket', async () => {
    const port = await freePort();
    await srv.setLan(true, port);
    expect(srv.lanServer!.requestTimeout).toBe(0);
    expect(srv.lanServer!.keepAliveTimeout).toBe(72_000);
    expect(srv.lanServer!.headersTimeout).toBe(60_000);
    const lan = `http://127.0.0.1:${port}`;

    const viaLan = new FakePhone(lan, PHONE_ID, phone.folders, { token: phone.token });
    viaLan.virtual = phone.virtual;
    expect((await viaLan.connect()).t).toBe('welcome');

    const lanEvents: any[] = [];
    const evWs = new WebSocket(`ws://127.0.0.1:${port}/api/events`);
    evWs.on('message', (d: Buffer) => lanEvents.push(JSON.parse(String(d))));
    await new Promise((r) => evWs.on('open', r));

    // a second phone pairs through the LAN; the LAN client never gets the pairing request and cannot approve
    const p3 = new FakePhone(lan, 'bbbbbbbb-0000-4000-8000-000000000003', [{ key: 'z', name: 'Z', root: path.join(tmp, 'phone') }], { name: 'Tablet' });
    const pend = await p3.connect();
    expect(pend.t).toBe('pending');
    await sleep(100);
    expect(lanEvents.some((e) => e.type === 'pair')).toBe(false);
    expect(events.some((e) => e.type === 'pair' && e.device.id === p3.id)).toBe(true);
    expect((await fetch(`${lan}/api/devices/${p3.id}/approve`, json('POST', {}))).status).toBe(403);
    expect((await api(`/api/devices/${p3.id}/approve`, json('POST', {}))).status).toBe(200);
    await p3.waitFrame((f) => f.t === 'paired');

    // LAN clients that are not paired phones see no device folders
    const anon = await j(await fetch(`${lan}/api/libraries`));
    expect(anon.devices).toEqual([]);
    expect(await j(await fetch(`${lan}/api/devices`))).toEqual([]);
    expect((await fetch(`${lan}/api/browse?folderId=${devFolder}`)).status).toBe(404);
    expect((await fetch(`${lan}/api/media/${vidId}/file`)).status).toBe(404);
    // albums holding phone items: LAN viewers get neither the items nor their map points
    const photoId = (await j(await api(`/api/browse?folderId=${libFolder}`))).items[0].id;
    const al = await j(await api('/api/albums', json('POST', { name: 'Mixed', ids: [photoId, imgId] })));
    srv.ctx.db.run('UPDATE media SET lat = 41, lon = 29 WHERE id IN (?, ?)', Number(photoId), Number(imgId));
    srv.ctx.events.bump();
    expect((await j(await api(`/api/geo/points?albumId=${al.id}`))).points.map((p: string[]) => p[0]).sort()).toEqual([photoId, imgId].sort());
    expect((await j(await fetch(`${lan}/api/browse?albumId=${al.id}`))).items.map((i: any) => i.id)).toEqual([photoId]);
    expect((await j(await fetch(`${lan}/api/geo/points?albumId=${al.id}`))).points.map((p: string[]) => p[0])).toEqual([photoId]);
    srv.ctx.db.run('UPDATE media SET lat = NULL, lon = NULL WHERE id IN (?, ?)', Number(photoId), Number(imgId));
    await api(`/api/albums/${al.id}`, { method: 'DELETE' });
    // a paired phone does (but not its own)
    const auth = { Authorization: `Bearer ${p3.token}`, 'X-Peek-Device': p3.id };
    const seen = (await j(await fetch(`${lan}/api/libraries`, { headers: auth }))).devices as DeviceInfo[];
    expect(seen.map((d) => d.id)).toEqual([PHONE_ID]);
    const ranged = await fetch(`${lan}/api/media/${vidId}/file`, { headers: { ...auth, Range: 'bytes=100-199' } });
    expect(ranged.status).toBe(206);
    expect(Buffer.from(await ranged.arrayBuffer()).equals(vidBytes.subarray(100, 200))).toBe(true);
    // <img>/<video> on the phone cannot send headers: ?pdev=&ptok= works as an alternative
    const qAuth = `pdev=${encodeURIComponent(p3.id)}&ptok=${encodeURIComponent(p3.token!)}`;
    const viaQuery = await fetch(`${lan}/api/media/${vidId}/file?${qAuth}`, { headers: { Range: 'bytes=0-9' } });
    expect(viaQuery.status).toBe(206);
    expect(Buffer.from(await viaQuery.arrayBuffer()).equals(vidBytes.subarray(0, 10))).toBe(true);
    expect((await j(await fetch(`${lan}/api/libraries?${qAuth}`))).devices.map((d: DeviceInfo) => d.id)).toEqual([PHONE_ID]);
    // a wrong query token sees nothing; headers take precedence over the query
    expect((await fetch(`${lan}/api/media/${vidId}/file?pdev=${p3.id}&ptok=${'z'.repeat(43)}`)).status).toBe(404);
    expect((await fetch(`${lan}/api/media/${vidId}/file?${qAuth}`, { headers: { 'X-Peek-Device': p3.id } })).status).toBe(404);
    // the query form identifies the device too: it never sees its own folders through it
    const ownQ = `pdev=${encodeURIComponent(PHONE_ID)}&ptok=${encodeURIComponent(viaLan.token!)}`;
    expect((await fetch(`${lan}/api/browse?folderId=${devFolder}&${ownQ}`)).status).toBe(404);

    // a long reply is in flight while sharing stops
    const bigId = (await j(await api(`/api/browse?folderId=${devFolder}&dir=Big`))).items.find((i: any) => i.name === 'large.mp4').id;
    let got = 0;
    let inflightRes: http.IncomingMessage | null = null;
    const inflight = new Promise<string>((resolve) => {
      http
        .get(`${lan}/api/media/${bigId}/file`, { headers: auth }, (res) => {
          inflightRes = res;
          res.pause();
          res.on('data', (c: Buffer) => (got += c.length));
          res.on('end', () => resolve('complete'));
          res.on('close', () => resolve('closed'));
          res.on('error', () => resolve('closed'));
        })
        .on('error', () => resolve('closed'));
    });
    await sleep(200);

    const evClosed = new Promise((r) => evWs.on('close', r));
    await srv.setLan(false);
    expect(srv.lanServer).toBeNull();
    await evClosed;
    expect(await viaLan.closed()).toBeGreaterThan(0);
    expect(await p3.closed()).toBeGreaterThan(0);
    // a paused client only notices the cut once it reads again
    await until(() => inflightRes);
    inflightRes!.resume();
    expect(await inflight).toBe('closed');
    expect(got).toBeLessThan(50 * 1024 * 1024);
    await until(async () => ((await j(await api('/api/devices'))) as DeviceInfo[]).every((d) => !d.online));
    await expect(fetch(`${lan}/api/info`)).rejects.toThrow();
  });

  it('forget removes the device, its folders and rows', async () => {
    expect((await api(`/api/devices/${PHONE_ID}`, { method: 'DELETE' })).status).toBe(200);
    expect(srv.ctx.db.get<{ c: number }>('SELECT count(*) c FROM folders WHERE device_id = ?', PHONE_ID)!.c).toBe(0);
    expect(srv.ctx.db.get<{ c: number }>('SELECT count(*) c FROM media WHERE id = ?', Number(imgId))!.c).toBe(0);
    expect(((await j(await api('/api/devices'))) as DeviceInfo[]).map((d) => d.id)).not.toContain(PHONE_ID);
  });
});

describe('linked phones on a headless server (mode "server")', () => {
  let srv: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-dev-srv-'));
  const root = path.join(tmp, 'phone');

  beforeAll(async () => {
    fs.mkdirSync(path.join(root, 'A'), { recursive: true });
    await sharp({ create: { width: 64, height: 48, channels: 3, background: '#123456' } }).jpeg().toFile(path.join(root, 'A', 'x.jpg'));
    srv = await createServer({ mode: 'server', dataDir: path.join(tmp, 'data'), port: 0, host: '127.0.0.1', mdns: false });
  });
  afterAll(async () => {
    await srv?.close();
  });

  it('any web client gets the pairing request and may approve; everyone sees device folders', async () => {
    const evs: any[] = [];
    const ws = new WebSocket(`${srv.url.replace('http', 'ws')}/api/events`);
    ws.on('message', (d: Buffer) => evs.push(JSON.parse(String(d))));
    await new Promise((r) => ws.on('open', r));
    const p = new FakePhone(srv.url, 'cccccccc-0000-4000-8000-000000000004', [{ key: 'cam', name: 'Camera', root }]);
    expect((await p.connect()).t).toBe('pending');
    await until(() => evs.find((e) => e.type === 'pair'));
    expect((await fetch(`${srv.url}/api/devices/${p.id}/approve`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status).toBe(200);
    await p.waitFrame((f) => f.t === 'paired');
    const devs = await until(async () => {
      const d = (await (await fetch(`${srv.url}/api/libraries`)).json()) as { devices: DeviceInfo[] };
      return d.devices[0]?.folders[0]?.mediaCount === 1 ? d.devices : null;
    });
    const b = (await (await fetch(`${srv.url}/api/browse?folderId=${devs[0].folders[0].id}&dir=A`)).json()) as { items: { name: string }[] };
    expect(b.items.map((i) => i.name)).toEqual(['x.jpg']);
    ws.close();
    p.close();
  });
});
