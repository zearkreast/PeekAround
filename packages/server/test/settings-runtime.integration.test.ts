import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { DEFAULT_SETTINGS } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import { hwAccelArgs, intensityLimits } from '../src/runtime-settings';

describe('runtime settings (thumb quality, indexing intensity, hwaccel) + cache routes', () => {
  let peek: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-rt-'));

  beforeAll(async () => {
    const lib = path.join(tmp, 'lib');
    fs.mkdirSync(lib, { recursive: true });
    peek = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false, mode: 'desktop' });
  });

  afterAll(async () => {
    await peek?.close();
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  const patch = (body: object) => peek.app.inject({ method: 'PATCH', url: '/api/settings', payload: body });

  it('has defaults for the new keys', async () => {
    const r = await peek.app.inject({ method: 'GET', url: '/api/settings' });
    const s = r.json();
    expect(s.thumbQuality).toBe(DEFAULT_SETTINGS.thumbQuality);
    expect(s.indexIntensity).toBe('normal');
    expect(s.hwAccel).toBe(false);
  });

  it('validates and applies them', async () => {
    let r = await patch({ thumbQuality: 'high', indexIntensity: 'low', hwAccel: true });
    expect(r.statusCode).toBe(200);
    expect(r.json()).toMatchObject({ thumbQuality: 'high', indexIntensity: 'low', hwAccel: true });
    expect(peek.ctx.thumbs.quality).toBe('high');
    expect(peek.ctx.thumbs.q(80)).toBe(88);
    expect(peek.ctx.thumbs.limiter.lowMax).toBe(1);
    expect(peek.ctx.scanner.concurrency).toEqual({ meta: 1, thumb: 1 });
    expect(peek.ctx.thumbs.bg.size).toBe(1);
    expect(peek.ctx.ffmpeg.decodeThreads).toBe(1);

    // invalid values are ignored (like every other key)
    r = await patch({ thumbQuality: 'ultra', indexIntensity: 9, hwAccel: 'yes' });
    expect(r.json()).toMatchObject({ thumbQuality: 'high', indexIntensity: 'low', hwAccel: true });

    r = await patch({ thumbQuality: 'low', indexIntensity: 'high', hwAccel: false });
    expect(peek.ctx.thumbs.q(80)).toBe(66);
    expect(peek.ctx.scanner.concurrency!.meta).toBe(intensityLimits('high').meta);
    expect(r.json().hwAccel).toBe(false);
  });

  it('maps intensity to concurrency', () => {
    // low: one background item at a time, single-threaded decodes, a pause after each item
    expect(intensityLimits('low', 8)).toMatchObject({ max: 4, lowMax: 1, workers: 1, vipsThreads: 1, ffmpegThreads: 1, meta: 1, thumb: 1 });
    expect(intensityLimits('low', 8).gapMs).toBeGreaterThan(0);
    // normal: about half of the cores, never all of them (libvips / ffmpeg single-threaded per item)
    expect(intensityLimits('normal', 8)).toMatchObject({ max: 6, lowMax: 3, workers: 3, gapMs: 0, vipsThreads: 1 });
    expect(intensityLimits('normal', 4)).toMatchObject({ workers: 2, lowMax: 2 });
    expect(intensityLimits('normal', 2).workers).toBe(1);
    // desktop app (server in Electron's main process): a quarter of the cores, at most 2
    expect(intensityLimits('normal', 4, true).workers).toBe(1);
    expect(intensityLimits('normal', 8, true).workers).toBe(2);
    expect(intensityLimits('normal', 32, true).workers).toBe(2);
    // high: all but one core, libvips threading on
    expect(intensityLimits('high', 16)).toMatchObject({ max: 12, lowMax: 8, workers: 8, vipsThreads: 0 });
    expect(intensityLimits('high', 1).workers).toBeGreaterThanOrEqual(1);
    for (const i of ['low', 'normal', 'high'] as const) for (const n of [1, 2, 4, 8, 32]) {
      const l = intensityLimits(i, n);
      expect(l.lowMax).toBeLessThanOrEqual(l.max);
      expect(l.workers).toBeLessThanOrEqual(Math.max(2, n));
    }
    expect(hwAccelArgs({ hwAccel: true })).toEqual(['-hwaccel', 'auto']);
    expect(hwAccelArgs({ hwAccel: false })).toEqual([]);
  });

  it('reports cache usage per kind and clears one kind', async () => {
    const thumbFile = path.join(peek.ctx.cfg.thumbDir, 'ab', 'abc-640.webp');
    fs.mkdirSync(path.dirname(thumbFile), { recursive: true });
    fs.writeFileSync(thumbFile, Buffer.alloc(1234));
    let r = await peek.app.inject({ method: 'GET', url: '/api/cache/details' });
    expect(r.statusCode).toBe(200);
    const d = r.json();
    expect(d.thumbs.bytes).toBe(1234);
    expect(d.tiles).toMatchObject({ bytes: 0, files: 0 });
    expect(typeof d.tiles.limitMb).toBe('number');
    expect(d.remote.limitMb).toBe(DEFAULT_SETTINGS.remoteCacheMb);

    r = await peek.app.inject({ method: 'DELETE', url: '/api/cache/bogus' });
    expect(r.statusCode).toBe(400);
    r = await peek.app.inject({ method: 'DELETE', url: '/api/cache/thumbs' });
    expect(r.statusCode).toBe(200);
    expect(fs.existsSync(thumbFile)).toBe(false);
    for (const k of ['tiles', 'remote']) expect((await peek.app.inject({ method: 'DELETE', url: `/api/cache/${k}` })).statusCode).toBe(200);
  });

  it('refuses clearing caches from the network (desktop LAN share)', async () => {
    const port = 18300 + Math.floor(Math.random() * 90);
    await peek.setLan(true, port);
    try {
      const del = await fetch(`http://127.0.0.1:${port}/api/cache/tiles`, { method: 'DELETE' });
      expect(del.status).toBe(403);
      const get = await fetch(`http://127.0.0.1:${port}/api/cache/details`);
      expect(get.status).toBe(200);
    } finally {
      await peek.setLan(false);
    }
  });
});
