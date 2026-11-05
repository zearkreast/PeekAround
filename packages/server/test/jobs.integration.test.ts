import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import type { Job } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app.js';

/**
 * End-to-end: real server (port 0), real ffmpeg. Generates tiny clips / images / music,
 * runs video edits and slideshow exports through the HTTP API and checks the results
 * with ffprobe.
 */

function hasFfmpeg(): boolean {
  try {
    execFileSync('ffmpeg', ['-version'], { stdio: 'ignore' });
    execFileSync('ffprobe', ['-version'], { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

const ff = (args: string[]) => execFileSync('ffmpeg', ['-hide_banner', '-v', 'error', '-y', ...args], { stdio: ['ignore', 'ignore', 'pipe'] });

let tmp: string;
let lib: string;
let server: PeekServer;
let folderId: number;
const ids: Record<string, string> = {};

async function pixel(png: Buffer, x: number, y: number): Promise<[number, number, number]> {
  const { data, info } = await sharp(png).removeAlpha().raw().toBuffer({ resolveWithObject: true });
  const o = (y * info.width + x) * info.channels;
  return [data[o], data[o + 1], data[o + 2]];
}

async function post(url: string, payload: unknown) {
  return server.app.inject({ method: 'POST', url, payload: payload as object });
}

async function waitJob(id: string, timeoutMs = 150_000): Promise<Job> {
  const t0 = Date.now();
  for (;;) {
    const res = await server.app.inject({ method: 'GET', url: '/api/jobs' });
    const job = (res.json() as { jobs: Job[] }).jobs.find((j) => j.id === id);
    if (!job) throw new Error('job vanished');
    if (job.status === 'done' || job.status === 'error' || job.status === 'canceled') return job;
    if (Date.now() - t0 > timeoutMs) throw new Error(`job ${id} timed out (${job.status} ${job.progress})`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

async function runJob(url: string, spec: unknown): Promise<{ job: Job; file: string }> {
  const res = await post(url, spec);
  expect(res.statusCode, res.body).toBe(200);
  const job = await waitJob((res.json() as Job).id);
  expect(job.error).toBeNull();
  expect(job.status).toBe('done');
  expect(job.result).not.toBeNull();
  const file = path.join(lib, ...job.result!.dir.split('/').filter(Boolean), job.result!.name);
  expect(fs.existsSync(file)).toBe(true);
  return { job, file };
}

async function probe(file: string) {
  const p = await server.ctx.ffmpeg.probe(file);
  expect(p).not.toBeNull();
  return p!;
}

function listParts(dir: string): string[] {
  return fs.readdirSync(dir).filter((n) => n.endsWith('.part'));
}

describe.skipIf(!hasFfmpeg())('jobs (integration)', () => {
  beforeAll(async () => {
    tmp = await fsp.mkdtemp(path.join(os.tmpdir(), 'peek-jobs-'));
    lib = path.join(tmp, 'lib');
    await fsp.mkdir(path.join(lib, 'out'), { recursive: true });

    // 5 s 640x480 test pattern with a 440 Hz tone
    ff(['-f', 'lavfi', '-i', 'testsrc2=s=640x480:r=25:d=5', '-f', 'lavfi', '-i', 'sine=f=440:d=5', '-c:v', 'libx264', '-preset', 'ultrafast', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-shortest', path.join(lib, 'clip.mp4')]);
    // 3 s, no audio stream
    ff(['-f', 'lavfi', '-i', 'testsrc=s=320x240:r=25:d=3', '-c:v', 'libx264', '-preset', 'ultrafast', '-pix_fmt', 'yuv420p', path.join(lib, 'silent.mp4')]);
    // phone-style rotated clip: stored 320x240, displayed 240x320
    try {
      ff(['-display_rotation', '-90', '-i', path.join(lib, 'silent.mp4'), '-c', 'copy', path.join(lib, 'rotated.mp4')]);
    } catch {
      /* ffmpeg < 6 has no -display_rotation — that test is skipped */
    }
    // 3 s of music (shorter than the slideshow → must loop)
    ff(['-f', 'lavfi', '-i', 'sine=f=330:d=3', '-c:a', 'aac', path.join(lib, 'music.m4a')]);

    const colors = ['#e74c3c', '#2ecc71', '#3498db'];
    for (let i = 0; i < 3; i++) {
      await sharp({ create: { width: 800, height: 600, channels: 3, background: colors[i] } })
        .composite([{ input: Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="800" height="600"><circle cx="400" cy="300" r="200" fill="white"/></svg>`) }])
        .jpeg()
        .toFile(path.join(lib, `photo${i + 1}.jpg`));
    }
    // landscape pixels + EXIF orientation 6 → displayed as a portrait image
    await sharp({ create: { width: 600, height: 400, channels: 3, background: '#f1c40f' } })
      .jpeg()
      .withMetadata({ orientation: 6 })
      .toFile(path.join(lib, 'portrait.jpg'));
    await fsp.writeFile(path.join(lib, 'notes.txt'), 'not media');

    server = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: lib }], port: 0, host: '127.0.0.1' });
    folderId = server.ctx.lib.allFolderRows()[0].id;
    for (const f of ['clip.mp4', 'silent.mp4', 'rotated.mp4', 'music.m4a', 'photo1.jpg', 'photo2.jpg', 'photo3.jpg', 'portrait.jpg']) {
      if (!fs.existsSync(path.join(lib, f))) continue;
      const id = await server.ctx.scanner.upsertFile(folderId, f);
      ids[f] = String(id);
    }
  });

  afterAll(async () => {
    if (server) {
      // let the background metadata/thumbnail workers finish before closing the db
      for (let i = 0; i < 200 && server.ctx.scanner.busy; i++) await new Promise((r) => setTimeout(r, 100));
      await server.close();
    }
    if (tmp) await fsp.rm(tmp, { recursive: true, force: true });
  });

  it('edits a video: trim 1–3 s, crop, speed 2, timed overlay', async () => {
    // overlay rendered at the OUTPUT resolution (crop 320x240): red box top-left
    const png = await sharp({ create: { width: 320, height: 240, channels: 4, background: { r: 0, g: 0, b: 0, alpha: 0 } } })
      .composite([
        {
          input: Buffer.from(
            `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="240"><rect x="0" y="0" width="120" height="60" fill="#ff0000"/><text x="10" y="200" font-size="40" fill="white">Hi</text></svg>`,
          ),
        },
      ])
      .png()
      .toBuffer();
    const assetId = await server.ctx.jobs.saveAsset(png, 'png');

    const { job, file } = await runJob('/api/jobs/video-edit', {
      sourceId: ids['clip.mp4'],
      trim: { start: 1, end: 3 },
      crop: { x: 100, y: 60, w: 320, h: 240 },
      speed: 2,
      volume: 0.8,
      adjust: { brightness: 0.1, saturation: 0.2, warmth: 0.3, vignette: 0.3 },
      overlays: [{ assetId, start: 0, end: 0.5 }],
      output: { format: 'mp4', height: null, quality: 'medium' },
    });
    expect(job.title).toBe('clip.mp4');
    expect(job.result).toEqual({ folderId: String(folderId), dir: '', name: 'clip_edit.mp4' });
    const p = await probe(file);
    expect(p.duration!).toBeGreaterThan(0.7);
    expect(p.duration!).toBeLessThan(1.3);
    expect([p.width, p.height]).toEqual([320, 240]);
    expect(p.codec).toBe('h264');
    expect(p.audioCodec).toBe('aac');

    // overlay visible during 0–0.5 s of the output, gone afterwards
    const [r1, g1, b1] = await pixel(await server.ctx.ffmpeg.frame(file, 0.2), 30, 20);
    expect(r1).toBeGreaterThan(200);
    expect(g1 + b1).toBeLessThan(120);
    const [r2, g2, b2] = await pixel(await server.ctx.ffmpeg.frame(file, 0.8), 30, 20);
    expect(r2 > 200 && g2 + b2 < 120).toBe(false);

    // indexed, original untouched, no temp leftovers
    const row = server.ctx.db.get<{ id: number }>('SELECT id FROM media WHERE folder_id = ? AND dir = ? AND name = ?', folderId, '', 'clip_edit.mp4');
    expect(row).toBeTruthy();
    expect((await probe(path.join(lib, 'clip.mp4'))).duration!).toBeCloseTo(5, 0);
    expect(listParts(lib)).toEqual([]);
  });

  it('second edit gets a new name; webm without audio', async () => {
    const { file } = await runJob('/api/jobs/video-edit', {
      sourceId: ids['clip.mp4'],
      trim: { start: 0, end: 1.5 },
      mute: true,
      rotate: 90,
      flipH: true,
      output: { format: 'webm', height: 240, quality: 'low' },
    });
    expect(path.basename(file)).toBe('clip_edit.webm');
    const p = await probe(file);
    expect([p.width, p.height]).toEqual([180, 240]);
    expect(p.duration!).toBeCloseTo(1.5, 0);
    expect(p.hasAudio).toBe(false);
    expect(['vp9', 'vp8']).toContain(p.codec);

    const again = await runJob('/api/jobs/video-edit', {
      sourceId: ids['clip.mp4'],
      trim: { start: 0, end: 1 },
      output: { format: 'mp4', height: 120, quality: 'low' },
    });
    expect(path.basename(again.file)).toBe('clip_edit-2.mp4');
  });

  it('exports a gif from a clip without audio (slow-motion)', async () => {
    const { file } = await runJob('/api/jobs/video-edit', {
      sourceId: ids['silent.mp4'],
      trim: { start: 0.5, end: 1.5 },
      speed: 0.5,
      output: { format: 'gif', height: 120, quality: 'high' },
    });
    expect(path.basename(file)).toBe('silent_edit.gif');
    const p = await probe(file);
    expect(p.codec).toBe('gif');
    expect([p.width, p.height]).toEqual([160, 120]);
    expect(Math.abs(p.duration! - 2)).toBeLessThan(0.3);
  });

  it('crops rotated sources in displayed orientation', async () => {
    if (!ids['rotated.mp4']) return;
    const src = await probe(path.join(lib, 'rotated.mp4'));
    if (src.rotation === 0) return; // ffmpeg build could not tag the rotation
    expect([src.width, src.height]).toEqual([240, 320]);
    const { file } = await runJob('/api/jobs/video-edit', {
      sourceId: ids['rotated.mp4'],
      crop: { x: 0, y: 100, w: 240, h: 200 },
      output: { format: 'mp4', height: null, quality: 'low' },
    });
    const p = await probe(file);
    expect([p.width, p.height]).toEqual([240, 200]);
    expect(p.rotation).toBe(0);
  });

  it('rejects bad video edits with 400 before queueing', async () => {
    const before = server.ctx.jobs.list().length;
    const cases: [unknown, number][] = [
      [{ sourceId: ids['photo1.jpg'], output: { format: 'mp4', height: null, quality: 'high' } }, 400],
      [{ sourceId: ids['clip.mp4'], crop: { x: 700, y: 0, w: 10, h: 10 }, output: { format: 'mp4', height: null, quality: 'high' } }, 400],
      [{ sourceId: ids['clip.mp4'], trim: { start: 9, end: 12 }, output: { format: 'mp4', height: null, quality: 'high' } }, 400],
      [{ sourceId: ids['clip.mp4'], speed: 10, output: { format: 'mp4', height: null, quality: 'high' } }, 400],
      [
        {
          sourceId: ids['clip.mp4'],
          overlays: [{ assetId: '00000000-0000-0000-0000-000000000000.png', start: 0, end: 1 }],
          output: { format: 'mp4', height: null, quality: 'high' },
        },
        400,
      ],
      [{ sourceId: '999999', output: { format: 'mp4', height: null, quality: 'high' } }, 404],
    ];
    for (const [spec, code] of cases) {
      const res = await post('/api/jobs/video-edit', spec);
      expect(res.statusCode, res.body).toBe(code);
    }
    expect(server.ctx.jobs.list().length).toBe(before);
  });

  it('exports a 3-image Ken Burns slideshow with a transition and looped music', async () => {
    const { job, file } = await runJob('/api/jobs/slideshow', {
      items: [{ id: ids['photo1.jpg'] }, { id: ids['photo2.jpg'] }, { id: ids['photo3.jpg'] }],
      duration: 2,
      transition: 'circle',
      transitionDuration: 0.5,
      kenBurns: true,
      audioId: ids['music.m4a'],
      width: 640,
      height: 360,
      fps: 25,
      background: '#000000',
      target: { folderId: String(folderId), dir: 'out' },
      name: 'Tatil',
    });
    expect(job.title).toBe('Tatil.mp4');
    expect(job.result).toEqual({ folderId: String(folderId), dir: 'out', name: 'Tatil.mp4' });
    const p = await probe(file);
    // 3 × 2 s − 2 × 0.5 s
    expect(Math.abs(p.duration! - 5)).toBeLessThan(0.3);
    expect([p.width, p.height]).toEqual([640, 360]);
    expect(p.codec).toBe('h264');
    expect(p.hasAudio).toBe(true);
    expect(p.audioCodec).toBe('aac');
    expect(server.ctx.db.get('SELECT id FROM media WHERE folder_id = ? AND dir = ? AND name = ?', folderId, 'out', 'Tatil.mp4')).toBeTruthy();
    // no temp leftovers
    expect(listParts(path.join(lib, 'out'))).toEqual([]);
    expect(fs.readdirSync(server.ctx.cfg.tempDir).filter((n) => n.startsWith('slideshow-'))).toEqual([]);
  });

  it('slideshow with a video item, EXIF-rotated photo, hard cuts, default name/location', async () => {
    const { file, job } = await runJob('/api/jobs/slideshow', {
      items: [{ id: ids['photo1.jpg'] }, { id: ids['portrait.jpg'], duration: 1.5 }, { id: ids['clip.mp4'], trim: { start: 1, end: 2.5 } }],
      duration: 1,
      transition: 'none',
      transitionDuration: 0.5,
      kenBurns: false,
      width: 320,
      height: 180,
      fps: 20,
      background: '#202020',
    });
    expect(job.result).toEqual({ folderId: String(folderId), dir: '', name: 'Slayt.mp4' });
    const p = await probe(file);
    expect(Math.abs(p.duration! - 4)).toBeLessThan(0.3);
    expect([p.width, p.height]).toEqual([320, 180]);
    expect(p.hasAudio).toBe(false);
    // portrait photo (1.0–2.5 s): pillarboxed → background at the left edge, yellow in the middle
    const frame = await server.ctx.ffmpeg.frame(file, 1.7);
    const edge = await pixel(frame, 5, 90);
    const mid = await pixel(frame, 160, 90);
    expect(Math.max(...edge)).toBeLessThan(60);
    expect(mid[0]).toBeGreaterThan(180);
    expect(mid[1]).toBeGreaterThan(150);
    expect(mid[2]).toBeLessThan(90);
  });

  it('random transitions with a video slide', async () => {
    const { file } = await runJob('/api/jobs/slideshow', {
      items: [{ id: ids['silent.mp4'] }, { id: ids['photo2.jpg'] }, { id: ids['photo3.jpg'] }],
      duration: 1.5,
      transition: 'random',
      transitionDuration: 0.4,
      kenBurns: false,
      width: 320,
      height: 240,
      fps: 25,
      background: '#fff',
      name: 'Karışık',
    });
    const p = await probe(file);
    // 3 + 1.5 + 1.5 − 2 × 0.4
    expect(Math.abs(p.duration! - 5.2)).toBeLessThan(0.3);
    expect(path.basename(file)).toBe('Karışık.mp4');
  });

  it('rejects bad slideshows with 400', async () => {
    const base = { duration: 2, transition: 'fade', transitionDuration: 0.5, kenBurns: false, width: 640, height: 360, fps: 25, background: '#000' };
    for (const spec of [
      { ...base, items: [] },
      { ...base, items: [{ id: ids['music.m4a'] }] },
      { ...base, items: [{ id: ids['photo1.jpg'] }], audioId: ids['photo2.jpg'] },
      { ...base, items: [{ id: ids['photo1.jpg'] }], transition: 'spin' },
      { ...base, items: [{ id: ids['photo1.jpg'] }], target: { folderId: String(folderId), dir: 'missing' } },
      { ...base, items: [{ id: ids['photo1.jpg'] }], target: { folderId: String(folderId), dir: '../..' } },
    ]) {
      const res = await post('/api/jobs/slideshow', spec);
      expect(res.statusCode, res.body).toBe(400);
    }
  });

  it('cancels a running job and removes the partial output', async () => {
    const res = await post('/api/jobs/slideshow', {
      items: [{ id: ids['photo1.jpg'] }, { id: ids['photo2.jpg'] }, { id: ids['photo3.jpg'] }],
      duration: 30,
      transition: 'fade',
      transitionDuration: 1,
      kenBurns: true,
      width: 1280,
      height: 720,
      fps: 30,
      background: '#000',
      name: 'Canceled',
    });
    expect(res.statusCode).toBe(200);
    const id = (res.json() as Job).id;
    const t0 = Date.now();
    for (;;) {
      const j = server.ctx.jobs.get(id)!;
      if (j.status === 'running' && j.progress > 0) break;
      if (Date.now() - t0 > 60_000 || j.status === 'done' || j.status === 'error') throw new Error(`unexpected state ${j.status}`);
      await new Promise((r) => setTimeout(r, 50));
    }
    await server.app.inject({ method: 'DELETE', url: `/api/jobs/${id}` });
    const job = await waitJob(id);
    expect(job.status).toBe('canceled');
    expect(fs.existsSync(path.join(lib, 'Canceled.mp4'))).toBe(false);
    // cleanup happens in the runner's finally, right before the job is marked canceled
    expect(listParts(lib)).toEqual([]);
    expect(fs.readdirSync(server.ctx.cfg.tempDir).filter((n) => n.startsWith('slideshow-'))).toEqual([]);
  });
});
