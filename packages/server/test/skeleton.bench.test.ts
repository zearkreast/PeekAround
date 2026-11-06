import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import type { BrowseResult, BrowseSkeleton, ExploreResult } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import { META_V } from '../src/media/metadata';

/** shared CI runners (2 slow vCPUs, noisy neighbours) get looser time limits; locally the real ones apply */
const SLOW = process.env.CI ? 4 : 1;

const N = 100_000;

/** Synthetic 100k-row library (rows inserted directly, no files) to keep the list endpoints fast. */
describe('skeleton performance (100k rows)', () => {
  let srv: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-bench-'));

  beforeAll(async () => {
    fs.mkdirSync(path.join(tmp, 'lib'));
    srv = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: path.join(tmp, 'lib'), name: 'Big' }], port: 0, host: '127.0.0.1', mdns: false });
    await srv.ctx.scanner.scanAll();
    // the start-up scan may still be running (scanAll above then only queued a rescan): wait until the
    // scanner is idle, or its rescan would drop the synthetic rows (not on disk) while explore yields
    for (let i = 0; i < 400 && srv.ctx.scanner.busy; i++) await new Promise((r) => setTimeout(r, 25));
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
    expect(ms).toBeLessThan(300 * SLOW);
  });

  it('pages, filters and explore stay fast', async () => {
    const p = await timed('/api/browse?offset=50000&limit=600');
    expect((p.r.json() as BrowseResult).items).toHaveLength(600);
    expect(p.ms).toBeLessThan(150 * SLOW);
    const f = await timed('/api/browse/skeleton?place=TR-34&sort=name&order=asc');
    expect((f.r.json() as BrowseSkeleton).total).toBe(N / 4);
    expect(f.ms).toBeLessThan(300 * SLOW);
    const cached = await srv.app.inject('/api/browse/skeleton?place=TR-34&sort=name&order=asc');
    expect(cached.body).toBe(f.r.body);
    const g = await timed('/api/geo/points');
    expect(g.ms).toBeLessThan(300 * SLOW);
    const t0 = performance.now();
    const ex = (await srv.app.inject('/api/explore')).json() as ExploreResult;
    const exMs = performance.now() - t0;
    console.log(`page ${p.ms.toFixed(0)} ms, filtered skeleton ${f.ms.toFixed(0)} ms, geo ${g.ms.toFixed(0)} ms, explore (cold) ${exMs.toFixed(0)} ms`);
    expect(ex.places).toHaveLength(3);
    expect(exMs).toBeLessThan(2000 * SLOW);
    // insights come with the same pass
    expect(ex.stats!.photos + ex.stats!.videos).toBe(N);
    expect(ex.countries!.map((c) => c.cc).sort()).toEqual(['DE', 'TR']);
    expect(ex.noLocation).toBe(N / 4);
    expect(ex.calendarYears!.length).toBeGreaterThanOrEqual(10);
  });

  it('explore is computed without blocking the event loop for long', async () => {
    let worst = 0;
    let last = performance.now();
    const t = setInterval(() => {
      const n = performance.now();
      worst = Math.max(worst, n - last);
      last = n;
    }, 5);
    const r = await srv.ctx.explore.compute({}, Date.now());
    clearInterval(t);
    worst = Math.max(worst, performance.now() - last);
    console.log(`explore compute: longest event-loop block ${worst.toFixed(0)} ms`);
    expect(r.stats!.photos + r.stats!.videos).toBe(N);
    // one synchronous pass over 100k rows blocked for ~1 s (desktop: the window's IPC with it)
    expect(worst).toBeLessThan(500 * SLOW);
  });

  it('insight filters stay fast', async () => {
    const out: string[] = [];
    for (const q of ['gps=none', 'orient=landscape', 'daytime=night', 'daytime=golden', 'color=blue', 'dup=1', 'byAdded=1&sort=mtime', 'place=TR-34&city=Kad%C4%B1k%C3%B6y']) {
      const { ms, r } = await timed(`/api/browse/skeleton?${q}`);
      expect(r.statusCode).toBe(200);
      out.push(`${q} ${ms.toFixed(0)} ms`);
      expect(ms, q).toBeLessThan(400 * SLOW);
    }
    const c = await timed('/api/explore/calendar?year=2020');
    expect(c.ms).toBeLessThan(150 * SLOW);
    console.log(out.join(', '), `calendar ${c.ms.toFixed(0)} ms`);
  });
});
