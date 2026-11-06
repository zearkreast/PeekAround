import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { Readable } from 'node:stream';
import sharp from 'sharp';
import type { FolderRef, Job, Library, RemoteFolderCreated, ServerEvent } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import {
  b64url,
  buildFileAttrResponse,
  ctrIv,
  ctrRangeDecryptor,
  ctrXor,
  decryptFileAttr,
  encryptFileAttr,
  foldKey,
  parseFa,
  parseFileAttrResponse,
} from '../src/remote/crypto';
import { Lanes, parseRange } from '../src/remote/folders';
import type { MegaClient, MegaLink, MegaNode } from '../src/remote/mega';

const K22 = 'AbCdEfGhIjKlMnOpQrStUv';
const FIX = path.join(__dirname, '../../../fixtures/library');
const sha = (b: Buffer) => crypto.createHash('sha256').update(b).digest('hex');
const tick = (ms = 50) => new Promise((r) => setTimeout(r, ms));

async function until<T>(fn: () => T | Promise<T>, ms = 15_000): Promise<NonNullable<T>> {
  const t0 = Date.now();
  for (;;) {
    const v = await fn();
    if (v) return v as NonNullable<T>;
    if (Date.now() - t0 > ms) throw new Error('timeout');
    await tick(40);
  }
}

// ───────────────────────── crypto vectors ─────────────────────────

describe('MEGA crypto', () => {
  // key 00 01 … 1f → folded key 10×16, CTR nonce 10 11 … 17 (vectors computed with `openssl enc`)
  const key = Buffer.from(Array.from({ length: 32 }, (_, i) => i));

  it('folds node keys and builds CTR IVs', () => {
    expect(foldKey(key).toString('hex')).toBe('10'.repeat(16));
    expect(ctrIv(key, 0).toString('hex')).toBe('1011121314151617' + '0000000000000000');
    expect(ctrIv(key, 16 * 0x0102 + 5).toString('hex')).toBe('1011121314151617' + '0000000000000102');
  });

  it('decrypts file attributes (AES-128-CBC, zero IV, folded key) — known vector', () => {
    const ct = Buffer.from('82c0bc02c96142da6ccd036430b8cc20ce72409ce665615334d40e9b5b2098dbbfa34a6031035f3416d01aad3dea8d6c', 'hex');
    const pt = decryptFileAttr(key, ct);
    expect(pt.subarray(0, 33).toString()).toBe('PeekAround MEGA attribute vector!');
    expect(encryptFileAttr(key, Buffer.from('PeekAround MEGA attribute vector!')).equals(ct)).toBe(true);
  });

  it('CTR random access matches the known vector at every offset', async () => {
    const plain = Buffer.from('0123456789abcdefPeekAround CTR!!');
    const ct = Buffer.from('adb3ea5bc315626304eccbd0e47658c67bd2271b719dd12ffb1de916faf852a4', 'hex');
    expect(ctrXor(key, plain).equals(ct)).toBe(true);
    for (const [s, e] of [[0, 31], [3, 20], [16, 16], [17, 31], [15, 16]]) {
      const aligned = s - (s % 16);
      const out: Buffer[] = [];
      const d = ctrRangeDecryptor(key, aligned, s, e);
      // feed the ciphertext in odd chunks
      const src = ct.subarray(aligned);
      for (let i = 0; i < src.length; i += 5) d.write(src.subarray(i, i + 5));
      d.end();
      for await (const c of d) out.push(c as Buffer);
      expect(Buffer.concat(out).toString(), `${s}-${e}`).toBe(plain.subarray(s, e + 1).toString());
    }
  });

  it('parses fa strings and attribute POST responses; trims JPEG padding', () => {
    const h0 = Buffer.from('0102030405060708', 'hex');
    const h1 = Buffer.from('a1a2a3a4a5a6a7a8', 'hex');
    const fa = parseFa(`924:0*${b64url(h0)}/924:1*${b64url(h1)}/bad/12:8*x`);
    expect(fa.map((f) => [f.cluster, f.type, f.handle.toString('hex')])).toEqual([
      [924, 0, h0.toString('hex')],
      [924, 1, h1.toString('hex')],
    ]);
    const jpeg = Buffer.concat([Buffer.from([0xff, 0xd8]), crypto.randomBytes(37), Buffer.from([0xff, 0xd9])]);
    const body = buildFileAttrResponse([
      { handle: h0, data: encryptFileAttr(key, Buffer.from('other')) },
      { handle: h1, data: encryptFileAttr(key, jpeg) },
    ]);
    const parts = parseFileAttrResponse(body);
    expect([...parts.keys()]).toEqual([h0.toString('hex'), h1.toString('hex')]);
    expect(decryptFileAttr(key, parts.get(h1.toString('hex'))!).equals(jpeg)).toBe(true);
    // truncated trailing part is ignored
    expect(parseFileAttrResponse(body.subarray(0, body.length - 3)).size).toBe(1);
  });

  it('parses ranges like the browser sends them', () => {
    expect(parseRange(undefined, 100)).toBeNull();
    expect(parseRange('bytes=0-', 100)).toEqual([0, 99]);
    expect(parseRange('bytes=10-19', 100)).toEqual([10, 19]);
    expect(parseRange('bytes=90-500', 100)).toEqual([90, 99]);
    expect(parseRange('bytes=-10', 100)).toEqual([90, 99]);
    expect(parseRange('bytes=100-', 100)).toBe('invalid');
    expect(parseRange('bytes=5-2', 100)).toBe('invalid');
  });

  it('lanes: at most 4 transfers, background never above 2, high first', async () => {
    const l = new Lanes(4, 2);
    const order: string[] = [];
    const rel: (() => void)[] = [];
    for (let i = 0; i < 3; i++) void l.acquire('low').then((r) => (order.push(`low${i}`), rel.push(r)));
    await tick(5);
    expect(order).toEqual(['low0', 'low1']);
    for (let i = 0; i < 3; i++) void l.acquire('high').then((r) => (order.push(`high${i}`), rel.push(r)));
    await tick(5);
    expect(order).toEqual(['low0', 'low1', 'high0', 'high1']);
    rel.shift()!();
    await tick(5);
    expect(order.at(-1)).toBe('high2');
  });
});

// ───────────────────────── fake MEGA ─────────────────────────

interface FakeFile {
  name: string;
  data: Buffer;
  ts?: number;
  /** MEGA file attributes this node carries */
  attrs?: { 0?: Buffer; 1?: Buffer };
}
interface FakeDir {
  name: string;
  children: (FakeFile | FakeDir)[];
}

const stats = { reads: 0, readBytes: 0, fullReads: 0, fullBy: {} as Record<string, number>, attrs: [] as string[], loads: 0, quota: false, offline: false };

let nid = 0;
function build(spec: FakeFile | FakeDir): MegaNode {
  const id = `F${String(nid++).padStart(7, '0')}`;
  if ('children' in spec) {
    const kids = spec.children.map(build);
    return { id, name: spec.name, size: 0, directory: true, timestamp: 1_700_000_000_000, children: kids, download: () => Readable.from([]) };
  }
  const key = crypto.randomBytes(32);
  const cipher = ctrXor(key, spec.data);
  const handles: Record<number, Buffer> = {};
  const fa = Object.keys(spec.attrs ?? {})
    .map((t) => {
      handles[Number(t)] = crypto.randomBytes(8);
      return `7:${t}*${b64url(handles[Number(t)])}`;
    })
    .join('/');
  const quotaErr = () => Object.assign(new Error('Bandwidth limit reached: 1200 seconds until it resets'), { timeLimit: '1200' });
  return {
    id,
    name: spec.name,
    size: spec.data.length,
    directory: false,
    timestamp: spec.ts ?? 1_700_000_000_000,
    children: [],
    fa: fa || null,
    download: () => Readable.from([spec.data]),
    read({ start, end, signal }) {
      stats.reads++;
      if (stats.quota) {
        const r = new Readable({ read() {} });
        setTimeout(() => r.destroy(quotaErr()), 5);
        return r;
      }
      // like MEGA: ciphertext from the 16-byte aligned offset, decrypted with the real CTR code
      const aligned = start - (start % 16);
      const src = cipher.subarray(aligned, end + 1);
      const chunks: Buffer[] = [];
      for (let i = 0; i < src.length; i += 7777) chunks.push(src.subarray(i, i + 7777));
      const s = Readable.from(chunks);
      signal.addEventListener('abort', () => s.destroy(), { once: true });
      if (start === 0 && end === spec.data.length - 1) {
        stats.fullReads++;
        stats.fullBy[id] = (stats.fullBy[id] ?? 0) + 1;
      }
      const out = s.pipe(ctrRangeDecryptor(key, aligned, start, end));
      // count what is actually consumed (ffmpeg aborts open-ended ranges early)
      out.on('data', (c: Buffer) => (stats.readBytes += c.length));
      return out;
    },
    async attr(type) {
      stats.attrs.push(`${spec.name}:${type}`);
      if (stats.quota) throw quotaErr();
      const plain = spec.attrs?.[type];
      if (!plain) return null;
      // round trip through the wire format + real decryption
      const wire = buildFileAttrResponse([{ handle: handles[type], data: encryptFileAttr(key, plain) }]);
      return decryptFileAttr(key, parseFileAttrResponse(wire).get(handles[type].toString('hex'))!);
    },
  };
}

// ───────────────────────── server ─────────────────────────

describe('remote (streaming) MEGA folders', () => {
  let srv: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-remote-'));
  const lib = path.join(tmp, 'lib');
  const trees: Record<string, () => MegaNode> = {};
  const client: MegaClient = {
    async load(link: MegaLink) {
      stats.loads++;
      if (stats.offline) throw new Error('fetch failed');
      if (link.id === 'Gone1234') throw new Error('ENOENT (-9): Object (typically, node or user) not found');
      const t = trees[link.id];
      if (!t) throw new Error('ENOENT (-9)');
      return t();
    },
  };
  const url = (id: string) => `https://mega.nz/folder/${id}#${K22}`;
  const inject = (method: string, p: string, body?: unknown, headers?: Record<string, string>) =>
    srv.app.inject({ method: method as 'GET', url: p, payload: body as object, headers });
  const events: ServerEvent[] = [];

  let big: Buffer;
  let jpgGps: Buffer;
  let preview: Buffer;
  let square: Buffer;
  let video: Buffer;
  let folder: FolderRef;
  let library: Library;
  let holidayFiles: (FakeFile | FakeDir)[];

  const rows = () => srv.ctx.db.all<any>('SELECT * FROM media WHERE folder_id = ? ORDER BY dir, name', Number(folder.id));
  const rowOf = (name: string) => rows().find((r) => r.name === name);

  beforeAll(async () => {
    fs.mkdirSync(path.join(lib, 'Local'), { recursive: true });
    jpgGps = fs.readFileSync(path.join(FIX, 'Fotoğraflar/2024 Yaz Tatili/IMG_2400.jpg'));
    // 1 MB "photo": EXIF at the start, the rest is never needed for metadata
    big = Buffer.concat([jpgGps, crypto.randomBytes(1024 * 1024)]);
    preview = await sharp({ create: { width: 400, height: 300, channels: 3, background: '#e11d48' } }).jpeg().toBuffer();
    square = await sharp({ create: { width: 120, height: 120, channels: 3, background: '#2563eb' } }).jpeg().toBuffer();
    video = fs.readFileSync(path.join(FIX, 'Videolar/dikey_klip.mp4'));
    holidayFiles = [
      { name: 'big.jpg', data: big, attrs: { 1: preview, 0: square } },
      { name: 'sq.jpg', data: jpgGps, attrs: { 0: square } },
      { name: 'plain.jpg', data: jpgGps, ts: 1_650_000_000_000 },
      { name: 'Sub', children: [{ name: 'clip.mp4', data: video }, { name: 'notes.txt', data: Buffer.from('x') }, { name: 'Empty', children: [] }] },
      // duplicate names exist on MEGA
      { name: 'dup.jpg', data: jpgGps },
      { name: 'dup.jpg', data: big },
    ];
    trees.Holiday1 = () => build({ name: 'Holiday', children: holidayFiles });
    srv = await createServer({ mode: 'desktop', dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false, mega: client });
    srv.ctx.events.intervals = { add: 1, remove: 1, change: 1, meta: 1, thumb: 1 };
    srv.ctx.events.on((e) => events.push(e));
    srv.ctx.remote.metaGapMs = 0;
  });

  afterAll(async () => {
    await srv?.close();
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it('adds a MEGA link as a new library: rows without downloading anything', async () => {
    const r = await inject('POST', '/api/remote/folders', { url: url('Holiday1') });
    expect(r.statusCode, r.body).toBe(200);
    const res = r.json() as RemoteFolderCreated;
    library = res.library;
    folder = res.folder;
    expect(library.name).toBe('Holiday');
    expect(folder.path).toBe('remote://mega/Holiday1');
    expect(folder.remote).toMatchObject({ provider: 'mega', state: 'ok', label: 'mega.nz/folder/Holiday1', linkKind: 'folder' });
    expect(rows().map((x) => [x.dir, x.name, x.kind, x.size])).toEqual([
      ['', 'big.jpg', 'image', big.length],
      ['', 'dup (2).jpg', 'image', big.length],
      ['', 'dup.jpg', 'image', jpgGps.length],
      ['', 'plain.jpg', 'image', jpgGps.length],
      ['', 'sq.jpg', 'image', jpgGps.length],
      ['Sub', 'clip.mp4', 'video', video.length],
    ]);
    expect(rowOf('plain.jpg').mtime).toBe(1_650_000_000_000);
    // browse: sub folders from the index (incl. the empty one), no upload target
    const b = (await inject('GET', `/api/browse?folderId=${folder.id}`)).json();
    expect(b.folders.map((f: any) => f.name)).toEqual(['Sub']);
    expect(b.target).toBeNull();
    const sub = (await inject('GET', `/api/browse?folderId=${folder.id}&dir=Sub`)).json();
    expect(sub.folders.map((f: any) => f.name)).toEqual(['Empty']);
    // home includes remote media
    expect((await inject('GET', '/api/browse?limit=100')).json().items.some((i: any) => i.folderId === folder.id)).toBe(true);
  });

  it('never exposes the link key to clients (and LAN clients cannot read it)', async () => {
    const libs = await inject('GET', '/api/libraries');
    expect(libs.body).not.toContain(K22);
    expect(libs.body).toContain('mega.nz/folder/Holiday1');
    const det = await inject('GET', `/api/media/${rowOf('big.jpg').id}`);
    expect(det.body).not.toContain(K22);
    // the local desktop window may see it
    const local = await inject('GET', `/api/remote/folders/${folder.id}/link`);
    expect(local.json().url).toBe(url('Holiday1'));
    // the key is never part of a media path / thumbnail URL either
    expect(rows().every((r: any) => !String(r.dir).includes(K22))).toBe(true);
    // a LAN client may browse and view, but not read the key
    const lan = await srv.setLan(true, 0 as unknown as number).catch(() => null);
    const port = (srv.lanServer?.address() as { port: number } | null)?.port;
    if (lan && port) {
      const get = (p: string) =>
        new Promise<{ status: number; body: string }>((resolve, reject) => {
          http.get({ host: '127.0.0.1', port, path: p }, (res) => {
            let body = '';
            res.on('data', (c) => (body += c));
            res.on('end', () => resolve({ status: res.statusCode ?? 0, body }));
          }).on('error', reject);
        });
      const l = await get(`/api/remote/folders/${folder.id}/link`);
      expect(l.status).toBe(403);
      expect(l.body).not.toContain(K22);
      const ll = await get('/api/libraries');
      expect(ll.status).toBe(200);
      expect(ll.body).not.toContain(K22);
      await srv.setLan(false);
    }
  });

  it('serves byte-exact HTTP ranges (seekable video)', async () => {
    const id = rowOf('clip.mp4').id;
    const full = await inject('GET', `/api/media/${id}/file`);
    expect(full.statusCode).toBe(200);
    expect(full.headers['accept-ranges']).toBe('bytes');
    expect(full.headers['content-type']).toBe('video/mp4');
    expect(sha(full.rawPayload)).toBe(sha(video));
    for (const [s, e] of [[0, 0], [1, 15], [17, 100_017], [video.length - 33, video.length - 1], [12345, 12345]]) {
      const r = await inject('GET', `/api/media/${id}/file`, undefined, { range: `bytes=${s}-${e}` });
      expect(r.statusCode).toBe(206);
      expect(r.headers['content-range']).toBe(`bytes ${s}-${e}/${video.length}`);
      expect(Number(r.headers['content-length'])).toBe(e - s + 1);
      expect(sha(r.rawPayload), `${s}-${e}`).toBe(sha(video.subarray(s, e + 1)));
    }
    const open = await inject('GET', `/api/media/${id}/file`, undefined, { range: 'bytes=500000-' });
    expect(sha(open.rawPayload)).toBe(sha(video.subarray(500000)));
    const suffix = await inject('GET', `/api/media/${id}/file`, undefined, { range: 'bytes=-1000' });
    expect(sha(suffix.rawPayload)).toBe(sha(video.subarray(video.length - 1000)));
    const bad = await inject('GET', `/api/media/${id}/file`, undefined, { range: `bytes=${video.length}-` });
    expect(bad.statusCode).toBe(416);
    const head = await inject('HEAD', `/api/media/${id}/file`);
    expect(Number(head.headers['content-length'])).toBe(video.length);
    // a viewed image is fetched once into the LRU cache, then served locally
    const img = rowOf('dup (2).jpg').id;
    expect(sha((await inject('GET', `/api/media/${img}/file`)).rawPayload)).toBe(sha(big));
    const dupNode = await srv.ctx.remote.nodeOf(srv.ctx.lib.folderRow(folder.id), rowOf('dup (2).jpg'));
    const before = stats.fullBy[dupNode.id];
    expect(before).toBe(1);
    expect(sha((await inject('GET', `/api/media/${img}/file`)).rawPayload)).toBe(sha(big));
    const ranged = await inject('GET', `/api/media/${img}/file`, undefined, { range: 'bytes=100-199' });
    expect(sha(ranged.rawPayload)).toBe(sha(big.subarray(100, 200)));
    expect(stats.fullBy[dupNode.id]).toBe(1);
    expect((await inject('GET', '/api/cache')).json().remoteBytes).toBeGreaterThanOrEqual(big.length);
  });

  it('thumbnails: MEGA preview → 120px square → original; thumbhash like local files', async () => {
    await until(() => rows().every((r) => r.kind !== 'image' || r.thumb_state !== 0));
    const big1 = rowOf('big.jpg');
    const sq = rowOf('sq.jpg');
    const plain = rowOf('plain.jpg');
    expect(big1.thumb_state).toBe(1);
    expect(big1.thumbhash).toBeTruthy();
    expect(stats.attrs).toContain('big.jpg:1');
    expect(stats.attrs).not.toContain('big.jpg:0'); // preview was enough
    expect(stats.attrs).toContain('sq.jpg:0');
    expect(sq.thumbhash).toBeTruthy();
    expect(plain.thumb_state).toBe(1); // from the original (≤ 30 MB)
    const t = await inject('GET', `/api/media/${big1.id}/thumb?s=640`);
    expect(t.statusCode).toBe(200);
    const meta = await sharp(t.rawPayload).metadata();
    expect([meta.width, meta.height]).toEqual([400, 300]); // the preview, never enlarged
    const s320 = await sharp((await inject('GET', `/api/media/${sq.id}/thumb?s=320`)).rawPayload).metadata();
    expect(s320.width).toBe(s320.height);
  });

  it('metadata: EXIF/GPS from the first 256 KB only; video probe over loopback', async () => {
    await until(() => rows().every((r) => r.meta_state !== 3), 60_000);
    const b = rowOf('big.jpg');
    expect(b.lat).toBeCloseTo(36.90705, 4);
    expect(b.lon).toBeCloseTo(30.69155, 4);
    expect(b.make).toBeTruthy();
    expect(b.time_src).toBeGreaterThan(0); // EXIF capture time, not the MEGA timestamp
    expect([b.width, b.height]).toEqual([(await sharp(jpgGps).metadata()).width, (await sharp(jpgGps).metadata()).height]);
    // big.jpg has a MEGA preview: never downloaded as a whole for metadata or thumbnails
    expect(stats.attrs.filter((a) => a.startsWith('big.jpg')).length).toBeGreaterThan(0);
    const bigNode = await srv.ctx.remote.nodeOf(srv.ctx.lib.folderRow(folder.id), b);
    expect(stats.fullBy[bigNode.id] ?? 0).toBe(0);
    if (srv.ctx.ffmpeg.available) {
      const v = rowOf('clip.mp4');
      expect(v.duration).toBeGreaterThan(7);
      expect([v.width, v.height]).toEqual([720, 1280]);
      expect(v.lat).toBeCloseTo(41.0082, 3);
      expect(v.playback).toBe('direct');
      expect(JSON.parse(v.extra).codec).toBe('h264');
    }
    // places / map work unchanged
    const pts = (await inject('GET', `/api/geo/points?folderId=${folder.id}&recursive=1`)).json();
    expect(pts.points.length).toBeGreaterThanOrEqual(1);
  });

  it('is read-only: no delete/rename/move/upload/edit-save; the link can be renamed or removed', async () => {
    const ref = { folderId: folder.id, rel: 'big.jpg' };
    const local = srv.ctx.lib.listLibraries().find((l) => l.name === 'Lib')!.folders[0];
    const checks = [
      await inject('POST', '/api/fs/delete', { refs: [ref] }),
      await inject('POST', '/api/fs/rename', { ...ref, name: 'x.jpg' }),
      await inject('POST', '/api/fs/move', { refs: [ref], to: { folderId: local.id, dir: '' } }),
      await inject('POST', '/api/fs/move', { refs: [{ folderId: local.id, rel: 'Local' }], to: { folderId: folder.id, dir: '' } }),
      await inject('POST', '/api/fs/mkdir', { folderId: folder.id, dir: '', name: 'New' }),
      await srv.app.inject({ method: 'PUT', url: `/api/upload?folderId=${folder.id}&path=a.jpg`, payload: Buffer.from('x'), headers: { 'content-type': 'application/octet-stream' } }),
      await srv.app.inject({ method: 'POST', url: `/api/edits/save?sourceId=${rowOf('big.jpg').id}`, payload: preview, headers: { 'content-type': 'application/octet-stream' } }),
    ];
    for (const c of checks) expect(c.statusCode, c.body).toBe(403);
    expect(rows().length).toBe(6);
    expect((await inject('PATCH', `/api/folders/${folder.id}`, { name: 'Tatil (MEGA)' })).statusCode).toBe(200);
    expect(srv.ctx.lib.folderRow(folder.id).name).toBe('Tatil (MEGA)');
  });

  it('copies remote items to this computer (MEGA import job, no re-listing)', async () => {
    const local = srv.ctx.lib.listLibraries().find((l) => l.name === 'Lib')!.folders[0];
    const loads = stats.loads;
    const r = await inject('POST', '/api/fs/move', { refs: [{ folderId: folder.id, rel: 'big.jpg' }, { folderId: folder.id, rel: 'Sub' }], to: { folderId: local.id, dir: 'Local' }, copy: true });
    expect(r.statusCode, r.body).toBe(200);
    const job = r.json().job as Job;
    const done = await until(() => {
      const j = srv.ctx.jobs.get(job.id);
      return j && j.status !== 'queued' && j.status !== 'running' ? j : null;
    }, 30_000);
    expect(done.status, done.error ?? '').toBe('done');
    expect(sha(fs.readFileSync(path.join(lib, 'Local/big.jpg')))).toBe(sha(big));
    expect(sha(fs.readFileSync(path.join(lib, 'Local/Sub/clip.mp4')))).toBe(sha(video));
    expect(fs.existsSync(path.join(lib, 'Local/Sub/notes.txt'))).toBe(true);
    expect(stats.loads).toBe(loads);
    // by media id through the dedicated route
    const r2 = await inject('POST', '/api/remote/copy', { ids: [rowOf('plain.jpg').id], to: { folderId: local.id, dir: '' } });
    expect(r2.statusCode, r2.body).toBe(200);
    // into a remote folder: refused
    const r3 = await inject('POST', '/api/remote/copy', { ids: [rowOf('plain.jpg').id], to: { folderId: folder.id, dir: '' } });
    expect(r3.statusCode).toBe(403);
  });

  it('refresh: new nodes are added, removed ones deleted (with events)', async () => {
    events.length = 0;
    holidayFiles = holidayFiles.filter((f) => f.name !== 'sq.jpg');
    holidayFiles.push({ name: 'new.png', data: await sharp(preview).png().toBuffer() });
    const keep = rowOf('big.jpg');
    expect((await inject('POST', `/api/remote/folders/${folder.id}/refresh`)).statusCode).toBe(200);
    const names = rows().map((x) => x.name);
    expect(names).toContain('new.png');
    expect(names).not.toContain('sq.jpg');
    // unchanged rows keep their id, metadata and thumbnail
    const again = rowOf('big.jpg');
    expect([again.id, again.lat, again.thumb_state]).toEqual([keep.id, keep.lat, 1]);
    await until(() => events.some((e) => e.type === 'media' && e.folderId === folder.id && e.reason === 'add'));
    expect(srv.ctx.lib.folderRef(srv.ctx.lib.folderRow(folder.id)).remote?.listedAt).toBeGreaterThan(Date.now() - 10_000);
  });

  it('offline: rows stay, status says so; a removed link is reported', async () => {
    stats.offline = true;
    const r = await inject('POST', `/api/remote/folders/${folder.id}/refresh`);
    expect(r.statusCode).toBe(502);
    const ref = srv.ctx.lib.folderRef(srv.ctx.lib.folderRow(folder.id));
    expect(ref.remote).toMatchObject({ state: 'offline', code: 'network' });
    expect(ref.remote!.retryAt).toBeGreaterThan(Date.now());
    expect(rows().length).toBeGreaterThan(0);
    stats.offline = false;
    const gone = await inject('POST', '/api/remote/folders', { url: url('Gone1234'), libraryId: library.id });
    expect(gone.statusCode).toBe(404);
    expect((await inject('POST', '/api/remote/folders', { url: 'https://example.com/x' })).statusCode).toBe(400);
    await inject('POST', `/api/remote/folders/${folder.id}/refresh`);
    expect(srv.ctx.lib.folderRef(srv.ctx.lib.folderRow(folder.id)).remote?.state).toBe('ok');
  });

  it('transfer quota: status + retry time, then no more requests to MEGA', async () => {
    const id = rowOf('clip.mp4').id;
    stats.quota = true;
    try {
    const r = await inject('GET', `/api/media/${id}/file`, undefined, { range: 'bytes=0-99' }).catch((e) => ({ statusCode: -1, rawPayload: Buffer.alloc(0), body: String(e) }));
    // the error surfaces mid-stream (headers already sent) or up front
    expect(r.statusCode === 206 || r.statusCode === -1 ? r.rawPayload.length < 100 : r.statusCode === 429, `${r.statusCode} ${r.body}`).toBe(true);
    await until(() => srv.ctx.remote.quotaUntil > Date.now());
    const ref = srv.ctx.lib.folderRef(srv.ctx.lib.folderRow(folder.id));
    expect(ref.remote).toMatchObject({ state: 'quota', code: 'quota' });
    expect(ref.remote!.retryAt).toBeGreaterThan(Date.now() + 1000 * 1000);
    const reads = stats.reads;
    const again = await inject('GET', `/api/media/${id}/file`, undefined, { range: 'bytes=0-99' });
    expect(again.statusCode).toBe(429);
    expect(stats.reads).toBe(reads);
    // a zip of remote items during the quota fails cleanly (no crash, no MEGA request)
    const z = await inject('GET', `/api/download?ids=${id}`).catch((e) => ({ statusCode: -1, body: String(e) }));
    expect(z.statusCode).not.toBe(200 + 1);
    await tick(200);
    expect(stats.reads).toBe(reads);
    } finally {
      stats.quota = false;
      srv.ctx.remote.quotaUntil = 0;
    }
  });

  it('zip download streams remote entries', async () => {
    const ids = [rowOf('plain.jpg').id, rowOf('clip.mp4').id].join(',');
    const r = await inject('GET', `/api/download?ids=${ids}`);
    expect(r.statusCode).toBe(200);
    expect(r.rawPayload.length).toBeGreaterThan(video.length + jpgGps.length);
    const z = await inject('GET', `/api/download?folderId=${folder.id}&dir=Sub`);
    expect(z.statusCode).toBe(200);
    expect(z.rawPayload.includes(Buffer.from('clip.mp4'))).toBe(true);
  });

  it('an aborted zip download releases its MEGA transfer slots', async () => {
    const f = srv.ctx.lib.folderRow(folder.id);
    // the zip writer reaches the first entry, then the client goes away while it is still open
    const ids = [rowOf('clip.mp4').id, rowOf('big.jpg').id, rowOf('dup (2).jpg').id].join(',');
    for (let round = 0; round < 3; round++) {
      await new Promise<void>((resolve, reject) => {
        const req = http.get(`${srv.url}/api/download?ids=${ids}`, (res) => {
          res.once('data', () => {
            req.destroy();
            resolve();
          });
        });
        req.on('error', () => {});
        req.setTimeout(10_000, () => reject(new Error('no data')));
      });
    }
    // same for a plain (video) stream the client stops reading
    for (let round = 0; round < 5; round++) {
      await new Promise<void>((resolve, reject) => {
        const req = http.get(`${srv.url}/api/media/${rowOf('clip.mp4').id}/file`, (res) => {
          res.once('data', () => {
            req.destroy();
            resolve();
          });
        });
        req.on('error', () => {});
        req.setTimeout(10_000, () => reject(new Error('no data')));
      });
    }
    await until(() => srv.ctx.remote.laneStats(f).running === 0, 5000);
    expect(srv.ctx.remote.laneStats(f)).toEqual({ running: 0, waiting: 0 });
    // and the link still streams
    const r = await inject('GET', `/api/media/${rowOf('clip.mp4').id}/file`, undefined, { range: 'bytes=0-99' });
    expect(r.statusCode).toBe(206);
  });

  it('removing the link drops its rows only', async () => {
    const r = await inject('DELETE', `/api/folders/${folder.id}`);
    expect(r.statusCode).toBe(200);
    expect(srv.ctx.db.get<any>('SELECT count(*) c FROM media WHERE folder_id = ?', Number(folder.id))!.c).toBe(0);
    expect(fs.existsSync(path.join(lib, 'Local/big.jpg'))).toBe(true);
  });
});

describe('remote folders: shutdown with MEGA transfers in flight', () => {
  it('close() aborts stalled background transfers instead of waiting for them', async () => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-remote-close-'));
    let started = 0;
    let aborted = 0;
    const stalled = (name: string): MegaNode => ({
      id: `S${name.padStart(7, '0')}`,
      name,
      size: 500_000,
      directory: false,
      timestamp: 1_700_000_000_000,
      children: [],
      fa: null,
      download: () => new Readable({ read() {} }),
      // MEGA accepted the request but never sends a byte
      read({ signal }) {
        started++;
        const r = new Readable({ read() {} });
        signal.addEventListener('abort', () => (aborted++, r.destroy(Object.assign(new Error('aborted'), { name: 'AbortError' }))), { once: true });
        return r;
      },
    });
    const client: MegaClient = {
      async load() {
        return { id: 'Root0000', name: 'Slow', size: 0, directory: true, timestamp: 0, children: [stalled('a.jpg'), stalled('b.jpg')], download: () => Readable.from([]) };
      },
    };
    const srv = await createServer({ mode: 'desktop', dataDir: path.join(tmp, 'data'), roots: [], port: 0, host: '127.0.0.1', mdns: false, mega: client });
    srv.ctx.remote.metaGapMs = 0;
    const r = await srv.app.inject({ method: 'POST', url: '/api/remote/folders', payload: { url: `https://mega.nz/folder/Slow1234#${K22}` } });
    expect(r.statusCode, r.body).toBe(200);
    await until(() => started >= 2, 5000);
    const t0 = Date.now();
    await srv.close();
    expect(Date.now() - t0).toBeLessThan(5000);
    expect(aborted).toBeGreaterThanOrEqual(started);
    fs.rmSync(tmp, { recursive: true, force: true });
  }, 30_000);
});
