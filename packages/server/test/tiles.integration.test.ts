import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import type { AddressInfo } from 'node:net';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { createServer, type PeekServer } from '../src/app';
import { tileService } from '../src/routes/tiles';
import { TileService } from '../src/tiles';

/** fake tile server: counts hits, configurable status / delay / body size */
function fakeUpstream() {
  const state = { hits: 0, status: 200, delay: 0, bytes: 2000, type: 'image/png', paths: [] as string[], ua: '' };
  const srv = http.createServer((req, res) => {
    state.hits++;
    state.paths.push(req.url ?? '');
    state.ua = String(req.headers['user-agent'] ?? '');
    setTimeout(() => {
      if (state.status !== 200) {
        res.writeHead(state.status, { 'Content-Type': 'text/plain' });
        res.end('nope');
        return;
      }
      res.writeHead(200, { 'Content-Type': state.type });
      res.end(Buffer.alloc(state.bytes, (req.url ?? '').length));
    }, state.delay);
  });
  return { srv, state };
}

describe('map tile proxy', () => {
  let peek: PeekServer;
  const up = fakeUpstream();
  let upUrl = '';
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-tiles-'));

  beforeAll(async () => {
    await new Promise<void>((r) => up.srv.listen(0, '127.0.0.1', () => r()));
    upUrl = `http://127.0.0.1:${(up.srv.address() as AddressInfo).port}`;
    peek = await createServer({ dataDir: path.join(tmp, 'data'), port: 0, host: '127.0.0.1', mdns: false, version: '9.9.9' });
    tileService(peek.ctx).upstream = (style, z, x, y) => `${upUrl}/${style}/${z}/${x}/${y}`;
  });

  afterAll(async () => {
    await peek?.close();
    await new Promise<void>((r) => up.srv.close(() => r()));
  });

  beforeEach(() => {
    Object.assign(up.state, { hits: 0, status: 200, delay: 0, bytes: 2000, type: 'image/png', paths: [] });
  });

  const tile = (url: string) => peek.app.inject({ method: 'GET', url });

  it('fetches once with an identifying User-Agent, then serves from the disk cache', async () => {
    const a = await tile('/api/tiles/osm/3/4/2.png');
    expect(a.statusCode).toBe(200);
    expect(a.headers['content-type']).toBe('image/png');
    expect(a.headers['x-peek-tile']).toBe('miss');
    expect(a.rawPayload.length).toBe(2000);
    expect(up.state.ua).toBe('PeekAround/9.9.9 (+https://github.com/zearkreast/PeakAround)');
    const b = await tile('/api/tiles/osm/3/4/2');
    expect(b.statusCode).toBe(200);
    expect(b.headers['x-peek-tile']).toBe('hit');
    expect(up.state.hits).toBe(1);
    expect(fs.existsSync(path.join(tmp, 'data', 'cache', 'tiles', 'osm', '3', '4', '2.png'))).toBe(true);
    // satellite tiles are JPEG with Esri's {z}/{y}/{x} order handled by the URL builder
    const s = await tile('/api/tiles/satellite/2/1/3.png');
    expect(s.headers['content-type']).toBe('image/jpeg');
  });

  it('coalesces concurrent requests for one tile into a single upstream fetch', async () => {
    up.state.delay = 150;
    const rs = await Promise.all(Array.from({ length: 8 }, () => tile('/api/tiles/osm/5/10/12.png')));
    expect(rs.every((r) => r.statusCode === 200)).toBe(true);
    expect(up.state.hits).toBe(1);
  });

  it('validates style and z/x/y ranges', async () => {
    for (const u of ['/api/tiles/osm/20/0/0.png', '/api/tiles/osm/3/8/0.png', '/api/tiles/osm/3/0/8.png', '/api/tiles/nope/1/0/0.png', '/api/tiles/osm/a/0/0.png', '/api/tiles/topo/18/0/0.png', '/api/tiles/osm/1/-1/0.png']) {
      const r = await tile(u);
      expect(r.statusCode, u).toBe(400);
    }
    expect(up.state.hits).toBe(0);
  });

  it('tells "refused" from "offline", and pauses a refusing upstream', async () => {
    up.state.status = 403;
    const r = await tile('/api/tiles/topo/4/1/1.png');
    expect(r.statusCode).toBe(502);
    expect(r.headers['x-peek-tile-error']).toBe('refused:403');
    expect(r.json()).toMatchObject({ reason: 'refused', status: 403 });
    // paused for a minute: no new upstream hit for another tile of that style
    const r2 = await tile('/api/tiles/topo/4/2/1.png');
    expect(r2.headers['x-peek-tile-error']).toBe('refused:403');
    expect(up.state.hits).toBe(1);
    const st = (await peek.app.inject('/api/tiles/status')).json();
    expect(st.topo).toMatchObject({ reason: 'refused', status: 403 });

    // offline: upstream unreachable
    const svc = tileService(peek.ctx);
    const old = svc.upstream;
    svc.upstream = () => 'http://127.0.0.1:1/x.png';
    const o = await tile('/api/tiles/osm/6/1/1.png');
    expect(o.statusCode).toBe(504);
    expect(o.headers['x-peek-tile-error']).toBe('offline');
    svc.upstream = old;
  });

  it('serves stale tiles when upstream fails (offline maps for viewed areas)', async () => {
    const ok = await tile('/api/tiles/osm/7/3/3.png');
    expect(ok.statusCode).toBe(200);
    // age the cached tile past the freshness window
    const f = path.join(tmp, 'data', 'cache', 'tiles', 'osm', '7', '3', '3.png');
    const old = new Date(Date.now() - 30 * 86_400_000);
    fs.utimesSync(f, old, old);
    const svc = tileService(peek.ctx);
    const keep = svc.upstream;
    svc.upstream = () => 'http://127.0.0.1:1/x.png';
    const s = await tile('/api/tiles/osm/7/3/3.png');
    expect(s.statusCode).toBe(200);
    expect(s.headers['x-peek-tile']).toBe('stale');
    expect(s.rawPayload.length).toBe(2000);
    svc.upstream = keep;
    // upstream back: revalidated (fresh copy)
    const n = await tile('/api/tiles/osm/7/3/3.png');
    expect(n.headers['x-peek-tile']).toBe('miss');
  });

  it('rejects non-image answers', async () => {
    up.state.type = 'text/html';
    const r = await tile('/api/tiles/satellite/3/3/3.png');
    expect(r.statusCode).toBe(502);
  });

  it('reports and clears the cache; settings carry style + cap', async () => {
    const st = (await peek.app.inject('/api/tiles/cache')).json();
    expect(st.files).toBeGreaterThan(0);
    expect(st.bytes).toBeGreaterThan(0);
    expect(st.limitMb).toBe(512);
    const s = await peek.app.inject({ method: 'PATCH', url: '/api/settings', payload: { mapTiles: 'satellite', tileCacheMb: 5 } });
    expect(s.json()).toMatchObject({ mapTiles: 'satellite', tileCacheMb: 32 });
    const bad = await peek.app.inject({ method: 'PATCH', url: '/api/settings', payload: { mapTiles: 'google' } });
    expect(bad.json().mapTiles).toBe('satellite');
    const c = await peek.app.inject({ method: 'DELETE', url: '/api/tiles/cache' });
    expect(c.json()).toMatchObject({ files: 0, bytes: 0 });
    expect((await tile('/api/tiles/osm/3/4/2.png')).headers['x-peek-tile']).toBe('miss');
  });

  it('tiles are not throttled like file-system routes', async () => {
    up.state.bytes = 10;
    // well above FS_LIMIT (240/min) and HEAVY_LIMIT (60/min)
    const rs = await Promise.all(Array.from({ length: 400 }, (_, i) => tile(`/api/tiles/osm/9/${i}/5.png`)));
    expect(rs.filter((r) => r.statusCode === 429)).toHaveLength(0);
    expect(rs.every((r) => r.statusCode === 200)).toBe(true);
    expect(rs[0].headers['x-ratelimit-limit']).toBe('20000');
  });
});

describe('tile cache LRU', () => {
  it('trims the least recently used tiles to 90 % of the cap', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-tilelru-'));
    let n = 0;
    const svc = new TileService({
      dir,
      userAgent: 'test',
      capMb: () => 1,
      fetch: (async () => {
        n++;
        return new Response(new Uint8Array(300 * 1024), { status: 200, headers: { 'Content-Type': 'image/png' } });
      }) as typeof fetch,
    });
    await svc.get('osm', 4, 0, 0);
    await new Promise((r) => setTimeout(r, 5));
    await svc.get('osm', 4, 1, 0);
    await new Promise((r) => setTimeout(r, 5));
    await svc.get('osm', 4, 2, 0);
    await new Promise((r) => setTimeout(r, 5));
    await svc.get('osm', 4, 0, 0); // touch → most recently used
    await new Promise((r) => setTimeout(r, 5));
    await svc.get('osm', 4, 3, 0); // 4 × 300 KiB > 1 MiB → trim
    await svc.trim();
    const st = await svc.stats();
    expect(st.bytes).toBeLessThanOrEqual(0.9 * 1024 * 1024);
    expect(fs.existsSync(path.join(dir, 'osm', '4', '1', '0.png'))).toBe(false); // oldest untouched
    expect(fs.existsSync(path.join(dir, 'osm', '4', '0', '0.png'))).toBe(true); // touched
    expect(fs.existsSync(path.join(dir, 'osm', '4', '3', '0.png'))).toBe(true); // newest
    expect(n).toBe(4);
    // a fresh service rebuilds its index from disk
    const again = new TileService({ dir, userAgent: 'test', capMb: () => 1 });
    expect((await again.stats()).files).toBe(st.files);
  });
});
