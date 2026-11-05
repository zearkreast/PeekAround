import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ServerEvent } from '@peekaround/shared';
import { EventHub } from '../src/events';
import { Ffmpeg } from '../src/ffmpeg';
import { Limiter, ThumbService } from '../src/media/thumbs';

describe('EventHub', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('coalesces and rate-limits media events per folder and reason', () => {
    vi.useFakeTimers();
    const hub = new EventHub();
    const got: ServerEvent[] = [];
    hub.on((e) => got.push(e));
    // a scan storm: 500 thumbnails + 500 meta updates in 20 s for one folder
    for (let t = 0; t < 20_000; t += 40) {
      hub.mediaChanged('1', `d${t % 3}`, 'thumb');
      hub.mediaChanged('1', 'd0', 'meta');
      vi.advanceTimersByTime(40);
    }
    vi.advanceTimersByTime(15_000);
    const thumbs = got.filter((e) => e.type === 'media' && e.reason === 'thumb');
    const metas = got.filter((e) => e.type === 'media' && e.reason === 'meta');
    expect(thumbs.length).toBeLessThanOrEqual(3); // ≤ 1 per 10 s
    expect(thumbs.length).toBeGreaterThanOrEqual(2);
    expect(metas.length).toBeLessThanOrEqual(8); // ≤ 1 per 3 s
    expect((thumbs[0] as { dirs: string[] }).dirs.sort()).toEqual(['d0', 'd1', 'd2']);
  });

  it('delivers add/remove almost immediately and bumps gen except for thumbnails', () => {
    vi.useFakeTimers();
    const hub = new EventHub();
    const got: ServerEvent[] = [];
    hub.on((e) => got.push(e));
    const g0 = hub.gen;
    hub.mediaChanged('1', '', 'thumb');
    expect(hub.gen).toBe(g0);
    hub.mediaChanged('1', 'a', 'add');
    hub.mediaChanged('2', '*', 'remove');
    expect(hub.gen).not.toBe(g0);
    vi.advanceTimersByTime(300);
    expect(got.filter((e) => e.type === 'media' && e.reason !== 'thumb')).toEqual([
      { type: 'media', folderId: '1', dirs: ['a'], reason: 'add' },
      { type: 'media', folderId: '2', dirs: ['*'], reason: 'remove' },
    ]);
    hub.close();
  });
});

describe('Limiter', () => {
  const tick = () => new Promise((r) => setTimeout(r, 5));

  it('runs high priority work first and keeps a slot free from background work', async () => {
    const lim = new Limiter(2);
    const order: string[] = [];
    const gate: (() => void)[] = [];
    const job = (name: string) => () =>
      new Promise<void>((res) => {
        order.push(name);
        gate.push(res);
      });
    for (let i = 0; i < 4; i++) lim.run(job(`low${i}`), 'low');
    await tick();
    expect(order).toEqual(['low0']); // low lane never takes the last slot
    lim.run(job('high0'), 'high');
    const b = lim.run(job('low-promoted'), 'low');
    b.promote();
    await tick();
    expect(order).toEqual(['low0', 'high0']);
    gate.shift()!();
    await tick();
    expect(order[2]).toBe('low-promoted');
  });
});

describe('ThumbService base dedup', () => {
  it('decodes a video frame once for 320, 640, 1600 and the background loop', async () => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-th-'));
    const vid = path.join(tmp, 'v.mp4');
    execFileSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc=size=320x240:rate=10', '-t', '2', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', vid]);
    const ff = new Ffmpeg('ffmpeg', 'ffprobe');
    await ff.check();
    const frames = vi.spyOn(ff, 'frame');
    const th = new ThumbService(path.join(tmp, 'thumbs'), ff);
    const row = { kind: 'video' as const, ext: 'mp4', mtime: 1, size: fs.statSync(vid).size, duration: 2 };
    const [a, b, c, d] = await Promise.all([
      th.generateBase(vid, row, 'low'),
      th.thumb(vid, row, 320),
      th.thumb(vid, row, 640),
      th.thumb(vid, row, 1600),
    ]);
    expect(a?.thumbhash).toBeTruthy();
    for (const f of [b, c, d]) expect(f && fs.existsSync(f)).toBe(true);
    expect(frames).toHaveBeenCalledTimes(1);
    // cached afterwards
    await th.thumb(vid, row, 320);
    expect(frames).toHaveBeenCalledTimes(1);
  });
});
