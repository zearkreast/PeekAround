import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Readable } from 'node:stream';
import yazl from 'yazl';
import type { Job, RemoteInspect } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import { megaError, parseMegaUrl, type MegaClient, type MegaLink, type MegaNode } from '../src/remote/mega';

const K22 = 'AbCdEfGhIjKlMnOpQrStUv';
const K43 = 'AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCd';

describe('MEGA link parser', () => {
  it('accepts every MEGA link format', () => {
    expect(parseMegaUrl(`https://mega.nz/file/AbCd1234#${K43}`)).toEqual({ type: 'file', id: 'AbCd1234', key: K43 });
    expect(parseMegaUrl(`https://mega.nz/folder/AbCd1234#${K22}`)).toEqual({ type: 'folder', id: 'AbCd1234', key: K22 });
    expect(parseMegaUrl(`https://mega.nz/folder/AbCd1234#${K22}/file/Zz9_-xYw`)).toEqual({ type: 'folder', id: 'AbCd1234', key: K22, sub: { type: 'file', id: 'Zz9_-xYw' } });
    expect(parseMegaUrl(`https://mega.nz/folder/AbCd1234#${K22}/folder/Qq1Ww2Ee`)).toEqual({ type: 'folder', id: 'AbCd1234', key: K22, sub: { type: 'folder', id: 'Qq1Ww2Ee' } });
    expect(parseMegaUrl(`https://mega.nz/#!AbCd1234!${K43}`)).toEqual({ type: 'file', id: 'AbCd1234', key: K43 });
    expect(parseMegaUrl(`https://mega.nz/#F!AbCd1234!${K22}`)).toEqual({ type: 'folder', id: 'AbCd1234', key: K22 });
    expect(parseMegaUrl(`https://mega.nz/#F!AbCd1234!${K22}!Qq1Ww2Ee`)).toEqual({ type: 'folder', id: 'AbCd1234', key: K22, sub: { type: 'node', id: 'Qq1Ww2Ee' } });
    expect(parseMegaUrl(`https://mega.co.nz/#!AbCd1234!${K43}`).type).toBe('file');
    expect(parseMegaUrl(`https://www.mega.nz/file/AbCd1234#${K43}`).type).toBe('file');
    expect(parseMegaUrl(`  https://mega.nz/file/AbCd1234#${K43}  `).id).toBe('AbCd1234');
  });

  it('rejects everything else with 400', () => {
    const bad = [
      '',
      'mega.nz/file/AbCd1234',
      `https://evil.com/file/AbCd1234#${K43}`,
      `https://mega.nz.evil.com/file/AbCd1234#${K43}`,
      `https://user:pw@mega.nz/file/AbCd1234#${K43}`,
      `https://mega.nz:8443/file/AbCd1234#${K43}`,
      `ftp://mega.nz/file/AbCd1234#${K43}`,
      `javascript:alert(1)//mega.nz/file/AbCd1234#${K43}`,
      'https://mega.nz/file/AbCd1234',
      `https://mega.nz/file/short#${K43}`,
      'https://mega.nz/file/AbCd1234#bad key!',
      `https://mega.nz/file/AbCd1234#${K43}/file/Zz9_-xYw`,
      `https://mega.nz/folder/AbCd1234#${K22}/file/../etc`,
      `https://mega.nz/folder/AbCd1234#${K22}/x/Zz9_-xYw`,
      `https://mega.nz/embed/AbCd1234#${K43}`,
      `https://mega.nz/#!AbCd1234`,
      `https://mega.nz/#F!AbCd1234!${K22}!a!b`,
      `https://mega.nz/`,
    ];
    for (const u of bad) {
      let status = 0;
      try {
        parseMegaUrl(u);
      } catch (e: any) {
        status = e.statusCode;
      }
      expect(status, u).toBe(400);
    }
    expect(() => parseMegaUrl(undefined as unknown as string)).toThrow();
  });

  it('maps MEGA errors to friendly messages', () => {
    const q = megaError(Object.assign(new Error('Bandwidth limit reached: 1800 seconds until it resets'), { timeLimit: '1800' })) as any;
    expect(q.statusCode).toBe(429);
    expect(q.message).toBe('MEGA bandwidth quota exceeded, try again later (retry in about 30 min)');
    expect(q.retryAfter).toBe(1800);
    expect(megaError(new Error('EOVERQUOTA (-17): Request over quota')).message).toMatch(/^MEGA bandwidth quota exceeded/);
    expect(megaError(new Error('MEGA returned a 509 status code')).statusCode).toBe(429);
    expect(megaError(new Error('ENOENT (-9): Object (typically, node or user) not found. Wrong password?')).message).toBe('MEGA link not found (removed, or the link is wrong)');
    expect(megaError(new Error('Attributes could not be decrypted with provided key.')).message).toBe('Invalid MEGA link key');
    expect(megaError(new Error('EKEY (-14): A decryption operation failed')).statusCode).toBe(400);
  });
});

// ───────────────────────── mocked MEGA client ─────────────────────────

const bytesOf = (seed: number, n: number) => Buffer.from(Array.from({ length: n }, (_, i) => (i * 7 + seed) & 0xff));

interface Spec {
  name: string;
  children?: Spec[];
  data?: Buffer;
  /** stream behaviour */
  slow?: boolean;
  fail?: Error;
}

let nextId = 0;
function node(s: Spec): MegaNode {
  const id = `N${String(nextId++).padStart(7, '0')}`;
  const kids = (s.children ?? []).map(node);
  return {
    id,
    name: s.name,
    size: s.data?.length ?? 0,
    directory: !!s.children,
    timestamp: 1_600_000_000_000,
    children: kids,
    download({ signal }) {
      if (s.fail) {
        const r = new Readable({ read() {} });
        setTimeout(() => r.destroy(s.fail), 20);
        return r;
      }
      if (s.slow) {
        let n = 0;
        const r = new Readable({
          read() {
            if (signal.aborted) return;
            setTimeout(() => (n++ < 10_000 ? r.push(Buffer.alloc(1024, 1)) : r.push(null)), 5);
          },
        });
        signal.addEventListener('abort', () => r.destroy(Object.assign(new Error('aborted'), { name: 'AbortError' })));
        return r;
      }
      return Readable.from([s.data!.subarray(0, 3), s.data!.subarray(3)]);
    },
  };
}

function find(n: MegaNode, name: string): MegaNode {
  if (n.name === name) return n;
  for (const c of n.children) {
    try {
      return find(c, name);
    } catch {
      /* next */
    }
  }
  throw new Error(`no ${name}`);
}

async function zipBuffer(): Promise<Buffer> {
  const z = new yazl.ZipFile();
  z.addBuffer(bytesOf(9, 300), 'inner/one.jpg');
  z.end();
  const chunks: Buffer[] = [];
  for await (const c of z.outputStream) chunks.push(c as Buffer);
  return Buffer.concat(chunks);
}

describe('MEGA inspect + import (mocked client)', () => {
  let srv: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-mega-'));
  const lib = path.join(tmp, 'lib');
  let trees: Record<string, MegaNode>;
  const loads: MegaLink[] = [];
  const client: MegaClient = {
    async load(link) {
      loads.push(link);
      if (link.id === 'Quota123') throw new Error('EOVERQUOTA (-17): Request over quota');
      if (link.id === 'Gone1234') throw new Error('ENOENT (-9): Object (typically, node or user) not found. Wrong password?');
      const t = trees[link.id];
      if (!t) throw new Error('ENOENT (-9)');
      return t;
    },
  };
  const url = (id: string, type = 'folder') => `https://mega.nz/${type}/${id}#${type === 'folder' ? K22 : K43}`;
  const post = (p: string, body: unknown) => srv.app.inject({ method: 'POST', url: p, payload: body as object });
  let zip: Buffer;

  beforeAll(async () => {
    fs.mkdirSync(path.join(lib, 'Target'), { recursive: true });
    zip = await zipBuffer();
    trees = {
      Holiday1: node({
        name: 'Holiday',
        children: [
          { name: 'a.jpg', data: bytesOf(1, 1000) },
          { name: 'Sub', children: [{ name: 'b.mp4', data: bytesOf(2, 2000) }, { name: 'notes.txt', data: bytesOf(3, 10) }] },
          { name: 'z.zip', data: zip },
        ],
      }),
      Single12: node({ name: 'clip.mp4', data: bytesOf(4, 500) }),
      Slowwww1: node({ name: 'Slow', children: [{ name: 'big.mp4', slow: true }] }),
      Mixed123: node({
        name: 'Mixed',
        children: [
          { name: 'ok.jpg', data: bytesOf(6, 100) },
          { name: 'bad.zip', data: Buffer.from('this is not a zip file') },
          { name: 'Deep', children: [{ name: 'lost.jpg', data: bytesOf(7, 10), fail: new Error('ECONNRESET') }] },
          { name: 'z.zip', data: zip },
        ],
      }),
      AllBad12: node({ name: 'AllBad', children: [{ name: 'Sub', children: [{ name: 'x.jpg', data: bytesOf(8, 10), fail: new Error('ECONNRESET') }] }, { name: 'y.zip', data: Buffer.from('nope') }] }),
      Broken12: node({ name: 'Broken', children: [{ name: 'q.jpg', data: bytesOf(5, 10), fail: Object.assign(new Error('Bandwidth limit reached: 600 seconds until it resets'), { timeLimit: '600' }) }] }),
    };
    srv = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false, mega: client });
  });

  afterAll(async () => {
    await srv?.close();
  });

  it('inspects a folder link', async () => {
    const r = await post('/api/remote/inspect', { url: url('Holiday1') });
    expect(r.statusCode).toBe(200);
    const ins = r.json() as RemoteInspect;
    expect(ins).toMatchObject({ provider: 'mega', kind: 'folder', name: 'Holiday', truncated: false, size: 1000 + 2000 + 10 + zip.length });
    expect(ins.nodes.map((n) => [n.path, n.dir, n.kind])).toEqual([
      ['', true, null],
      ['Sub', true, null],
      ['a.jpg', false, 'image'],
      ['z.zip', false, 'archive'],
      ['Sub/b.mp4', false, 'video'],
      ['Sub/notes.txt', false, null],
    ]);
    expect(ins.nodes.find((n) => n.path === 'Sub')!.size).toBe(2010);
    const one = (await post('/api/remote/inspect', { url: url('Single12', 'file') })).json() as RemoteInspect;
    expect(one).toMatchObject({ kind: 'file', name: 'clip.mp4', size: 500, nodes: [{ path: '', name: 'clip.mp4', kind: 'video', dir: false }] });
  });

  it('reports bad links and MEGA errors clearly', async () => {
    expect((await post('/api/remote/inspect', { url: 'https://example.com/x' })).statusCode).toBe(400);
    const q = await post('/api/remote/inspect', { url: url('Quota123') });
    expect(q.statusCode).toBe(429);
    expect(q.json().error).toMatch(/^MEGA bandwidth quota exceeded, try again later/);
    const g = await post('/api/remote/inspect', { url: url('Gone1234') });
    expect(g.statusCode).toBe(404);
    expect(g.json().error).toBe('MEGA link not found (removed, or the link is wrong)');
    expect((await post('/api/remote/import', { url: url('Holiday1'), mode: 'extract' })).statusCode).toBe(400);
    expect((await post('/api/remote/import', { url: url('Holiday1'), mode: 'weird', folderId: '1' })).statusCode).toBe(400);
  });

  it('imports selected nodes into a folder, preserving the tree, with progress', async () => {
    const h = trees.Holiday1;
    const seen: Job[] = [];
    const off = srv.ctx.events.on((e) => {
      if (e.type === 'job') seen.push(e.job);
    });
    const r = await post('/api/remote/import', { url: url('Holiday1'), nodes: [find(h, 'a.jpg').id, find(h, 'Sub').id], mode: 'extract', folderId: '1', dir: 'Target' });
    expect(r.statusCode).toBe(200);
    const job = r.json() as Job;
    expect(job.type).toBe('transfer');
    const fin = (await srv.ctx.jobs.wait(job.id))!;
    off();
    expect(fin.status).toBe('done');
    expect(fin.title).toBe('Holiday');
    expect(fin.bytes).toEqual({ done: 3010, total: 3010 });
    expect(fin.files).toEqual({ done: 3, total: 3 });
    expect(fin.result).toEqual({ folderId: '1', dir: 'Target', name: 'Holiday' });
    expect(seen.some((j) => j.status === 'running' && j.bytes && j.bytes.total === 3010)).toBe(true);
    const base = path.join(lib, 'Target', 'Holiday');
    expect(fs.readFileSync(path.join(base, 'a.jpg')).equals(bytesOf(1, 1000))).toBe(true);
    expect(fs.readFileSync(path.join(base, 'Sub', 'b.mp4')).equals(bytesOf(2, 2000))).toBe(true);
    expect(fs.existsSync(path.join(base, 'Sub', 'notes.txt'))).toBe(true);
    expect(fs.existsSync(path.join(base, 'z.zip'))).toBe(false);
    expect(fs.statSync(path.join(base, 'a.jpg')).mtimeMs).toBe(1_600_000_000_000);
    const b = (await srv.app.inject('/api/browse?folderId=1&dir=Target%2FHoliday&recursive=1')).json();
    expect(b.items.map((i: { name: string }) => i.name).sort()).toEqual(['a.jpg', 'b.mp4']);

    // a second import of the same folder lands next to it
    const again = (await post('/api/remote/import', { url: url('Holiday1'), nodes: [find(h, 'a.jpg').id], mode: 'extract', folderId: '1', dir: 'Target' })).json();
    expect((await srv.ctx.jobs.wait(again.id))!.result).toMatchObject({ name: 'Holiday (1)' });
  });

  it('handles zips inside the link per spec.zip (default from settings, ask → keep)', async () => {
    const zid = find(trees.Holiday1, 'z.zip').id;
    const run = async (zipMode: string | undefined, dir: string) => {
      fs.mkdirSync(path.join(lib, dir), { recursive: true });
      const j = (await post('/api/remote/import', { url: url('Holiday1'), nodes: [zid], mode: 'extract', folderId: '1', dir, ...(zipMode ? { zip: zipMode } : {}) })).json();
      const fin = (await srv.ctx.jobs.wait(j.id))!;
      expect(fin.status).toBe('done');
      return path.join(lib, dir, 'Holiday');
    };
    const ex = await run('extract', 'Z1');
    expect(fs.readFileSync(path.join(ex, 'z', 'inner', 'one.jpg')).equals(bytesOf(9, 300))).toBe(true);
    expect(fs.existsSync(path.join(ex, 'z.zip'))).toBe(false);
    const keep = await run(undefined, 'Z2');
    expect(fs.readFileSync(path.join(keep, 'z.zip')).equals(zip)).toBe(true);
    const before = (await srv.app.inject('/api/libraries')).json().temp.length;
    const tmpRun = await run('temp', 'Z3');
    expect(fs.existsSync(path.join(tmpRun, 'z.zip'))).toBe(false);
    const temps = (await srv.app.inject('/api/libraries')).json().temp;
    expect(temps.length).toBe(before + 1);
    expect(temps[0].sourceName).toBe('z.zip');
  });

  it('temp mode creates a temp session named after the link', async () => {
    const j = (await post('/api/remote/import', { url: url('Single12', 'file'), mode: 'temp' })).json();
    const fin = (await srv.ctx.jobs.wait(j.id))!;
    expect(fin.status).toBe('done');
    const temps = (await srv.app.inject('/api/libraries')).json().temp;
    const t = temps.find((x: { id: string }) => x.id === fin.result!.folderId);
    expect(t).toMatchObject({ name: 'clip.mp4', temp: true, sourceName: 'clip.mp4' });
    expect(fs.readFileSync(path.join(t.path, 'clip.mp4')).equals(bytesOf(4, 500))).toBe(true);
    const b = (await srv.app.inject(`/api/browse?folderId=${t.id}`)).json();
    expect(b.items.map((i: { name: string }) => i.name)).toEqual(['clip.mp4']);
  });

  it('cancels a running import (no .part left, empty temp session dropped)', async () => {
    const tempsBefore = (await srv.app.inject('/api/libraries')).json().temp.length;
    const j = (await post('/api/remote/import', { url: url('Slowwww1'), mode: 'temp' })).json();
    for (let i = 0; i < 100 && srv.ctx.jobs.get(j.id)!.bytes!.done < 4096; i++) await new Promise((r) => setTimeout(r, 20));
    expect((await srv.app.inject({ method: 'DELETE', url: `/api/jobs/${j.id}` })).statusCode).toBe(200);
    const fin = (await srv.ctx.jobs.wait(j.id))!;
    expect(fin.status).toBe('canceled');
    expect((await srv.app.inject('/api/libraries')).json().temp.length).toBe(tempsBefore);
    const parts: string[] = [];
    const walk = (d: string) => {
      for (const e of fs.readdirSync(d, { withFileTypes: true })) {
        if (e.isDirectory()) walk(path.join(d, e.name));
        else if (e.name.endsWith('.part')) parts.push(e.name);
      }
    };
    walk(tmp);
    expect(parts).toEqual([]);
  });

  it('a transfer does not wait for a running video job (separate lane)', async () => {
    let release!: () => void;
    const blocker = srv.ctx.jobs.enqueue('slideshow', 'blocker', () => new Promise((r) => (release = () => r(null))));
    const j = (await post('/api/remote/import', { url: url('Single12', 'file'), mode: 'extract', folderId: '1', dir: 'Lane' })).json();
    expect((await srv.ctx.jobs.wait(j.id))!.status).toBe('done');
    expect(srv.ctx.jobs.get(blocker.id)!.status).toBe('running');
    release();
    await srv.ctx.jobs.wait(blocker.id);
  });

  it('quota errors during a download fail the job with a friendly message', async () => {
    const j = (await post('/api/remote/import', { url: url('Broken12'), mode: 'extract', folderId: '1', dir: 'Target' })).json();
    const fin = (await srv.ctx.jobs.wait(j.id))!;
    expect(fin.status).toBe('error');
    expect(fin.error).toBe('MEGA bandwidth quota exceeded, try again later (retry in about 10 min)');
  });

  it('partial failures: other files and zips still land, job reports "n of m", no empty folders or leftovers', async () => {
    const tempDir = srv.ctx.cfg.tempDir;
    const sessionsBefore = fs.existsSync(tempDir) ? fs.readdirSync(tempDir).length : 0;
    fs.mkdirSync(path.join(lib, 'P1'), { recursive: true });
    const j = (await post('/api/remote/import', { url: url('Mixed123'), mode: 'extract', folderId: '1', dir: 'P1', zip: 'extract' })).json();
    const fin = (await srv.ctx.jobs.wait(j.id))!;
    expect(fin.status).toBe('error');
    expect(fin.error).toMatch(/^2 of 4 files could not be imported: /);
    expect(fin.result).toEqual({ folderId: '1', dir: 'P1', name: 'Mixed' });
    const base = path.join(lib, 'P1', 'Mixed');
    expect(fs.readFileSync(path.join(base, 'ok.jpg')).equals(bytesOf(6, 100))).toBe(true);
    expect(fs.readFileSync(path.join(base, 'z', 'inner', 'one.jpg')).equals(bytesOf(9, 300))).toBe(true);
    expect(fs.readdirSync(base).sort()).toEqual(['ok.jpg', 'z']); // no 'bad' folder, no empty 'Deep', no .part
    // temp zip mode: a broken zip leaves no orphan session dir
    fs.mkdirSync(path.join(lib, 'P2'), { recursive: true });
    const t = (await post('/api/remote/import', { url: url('Mixed123'), nodes: [find(trees.Mixed123, 'bad.zip').id], mode: 'extract', folderId: '1', dir: 'P2', zip: 'temp' })).json();
    const tf = (await srv.ctx.jobs.wait(t.id))!;
    expect(tf.status).toBe('error');
    expect(fs.readdirSync(path.join(lib, 'P2'))).toEqual([]);
    expect(fs.existsSync(tempDir) ? fs.readdirSync(tempDir).filter((n) => n !== 'uploads' && n !== 'job-assets').length : 0).toBeLessThanOrEqual(sessionsBefore);
  });

  it('a link where everything fails leaves no empty folder (extract and temp)', async () => {
    fs.mkdirSync(path.join(lib, 'P3'), { recursive: true });
    const j = (await post('/api/remote/import', { url: url('AllBad12'), mode: 'extract', folderId: '1', dir: 'P3', zip: 'extract' })).json();
    const fin = (await srv.ctx.jobs.wait(j.id))!;
    expect(fin.status).toBe('error');
    expect(fin.error).toMatch(/^2 of 2 files could not be imported/);
    expect(fin.result).toBeNull();
    expect(fs.readdirSync(path.join(lib, 'P3'))).toEqual([]);
    const temps = (await srv.app.inject('/api/libraries')).json().temp.length;
    const t = (await post('/api/remote/import', { url: url('AllBad12'), mode: 'temp', zip: 'extract' })).json();
    expect((await srv.ctx.jobs.wait(t.id))!.status).toBe('error');
    expect((await srv.app.inject('/api/libraries')).json().temp.length).toBe(temps);
    // quota failure of the only file: friendly message, no empty "Broken" folder
    expect(fs.existsSync(path.join(lib, 'Target', 'Broken'))).toBe(false);
  });
});
