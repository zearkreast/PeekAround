import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import type { BrowseResult, BrowseSkeleton, ExploreResult } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import { META_V } from '../src/media/metadata';

const N = 100_000;

/** Synthetic 100k-row library (rows inserted directly, no files) to keep the list endpoints fast. */
describe('skeleton performance (100k rows)', () => {
  let srv: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-bench-'));

  beforeAll(async () => {
    fs.mkdirSync(path.join(tmp, 'lib'));
    srv = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: path.join(tmp, 'lib'), name: 'Big' }], port: 0, host: '127.0.0.1', mdns: false });
    await srv.ctx.scanner.scanAll();
    const db = srv.ctx.db;
    let seed = 5;
    const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    const cities = [
      ['TR', '34', 'Kadıköy', 40.99, 29.03],
      ['TR', '07', 'Muratpaşa', 36.88, 30.7],
      ['DE', '16', 'Mitte', 52.52, 13.4],
      [null, null, null, null, null],
    ] as const;
    const ins = db.stmt(
      `INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, width, height, taken_at, taken_local, meta_state, thumb_state, meta_v, cc, admin1, city, lat, lon, make, model, flags, seen)
       VALUES (1, ?, ?, 'jpg', ?, 1000, ?, ?, ?, ?, ?, 1, 1, ${META_V}, ?, ?, ?, ?, ?, 'Apple', ?, ?, 1)`,
    );
    const start = Date.UTC(2015, 0, 1);
    db.tx(() => {
      for (let i = 0; i < N; i++) {
        const t = start + Math.floor(rnd() * 10 * 365 * 86_400_000);
        const c = cities[i % 4];
        ins.run(`y${i % 10}/m${i % 12}`, `IMG_${i}.jpg`, i % 20 ? 'image' : 'video', t, 3000 + (i % 3) * 1000, 3000, t, t + 3 * 3600_000, c[0], c[1], c[2], c[3], c[4], i % 2 ? 'iPhone 13' : 'iPhone 15', i % 50 === 0 ? 2 : 0);
      }
    });
  }, 120_000);

  afterAll(async () => {
    await srv?.close();
  });

  const timed = async (url: string) => {
    await srv.app.inject(url); // warm statement cache
    srv.ctx.events.bump(); // new generation → no skeleton cache hit
    const t0 = performance.now();
    const r = await srv.app.inject(url);
    return { ms: performance.now() - t0, r };
  };

  it('builds the full skeleton in < 300 ms', async () => {
    const { ms, r } = await timed('/api/browse/skeleton');
    const sk = r.json() as BrowseSkeleton;
    expect(sk.total).toBe(N);
    expect(Buffer.from(sk.aspects, 'base64').length).toBe(N);
    expect(sk.days!.length).toBeGreaterThan(3000);
    console.log(`skeleton 100k: ${ms.toFixed(0)} ms, ${(r.body.length / 1024).toFixed(0)} KB`);
    expect(ms).toBeLessThan(300);
  });

  it('pages, filters and explore stay fast', async () => {
    const p = await timed('/api/browse?offset=50000&limit=600');
    expect((p.r.json() as BrowseResult).items).toHaveLength(600);
    expect(p.ms).toBeLessThan(150);
    const f = await timed('/api/browse/skeleton?place=TR-34&sort=name&order=asc');
    expect((f.r.json() as BrowseSkeleton).total).toBe(N / 4);
    expect(f.ms).toBeLessThan(300);
    const cached = await srv.app.inject('/api/browse/skeleton?place=TR-34&sort=name&order=asc');
    expect(cached.body).toBe(f.r.body);
    const g = await timed('/api/geo/points');
    expect(g.ms).toBeLessThan(300);
    const t0 = performance.now();
    const ex = (await srv.app.inject('/api/explore')).json() as ExploreResult;
    const exMs = performance.now() - t0;
    console.log(`page ${p.ms.toFixed(0)} ms, filtered skeleton ${f.ms.toFixed(0)} ms, geo ${g.ms.toFixed(0)} ms, explore (cold) ${exMs.toFixed(0)} ms`);
    expect(ex.places).toHaveLength(3);
    expect(exMs).toBeLessThan(2000);
  });
});
