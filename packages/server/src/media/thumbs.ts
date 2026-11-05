import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import sharp, { type Sharp } from 'sharp';
import { rgbaToThumbHash } from 'thumbhash';
import type { Ffmpeg } from '../ffmpeg.js';
import type { MediaRow } from './mapper.js';

sharp.cache(false); // never keep file handles open (Windows can't rename/delete otherwise)

export const THUMB_SIZES = [320, 640, 1600] as const;
export type ThumbSize = (typeof THUMB_SIZES)[number];

export function snapSize(s: number): ThumbSize {
  for (const t of THUMB_SIZES) if (s <= t) return t;
  return 1600;
}

type ThumbRow = Pick<MediaRow, 'kind' | 'ext' | 'mtime' | 'size' | 'duration'>;
type BaseResult = { thumbhash: string | null; width: number; height: number } | null;

export type Priority = 'high' | 'low';

/**
 * Images of media that live on a linked phone (abs = `device://<id>/<key>/<rel>`), fetched through
 * the device tunnel. Results are cached exactly like local renditions (key = abs|mtime|size), so
 * cached thumbnails keep working while the phone is offline.
 */
export interface RemoteImageSource {
  /** JPEG thumbnail, long edge ≈ size (video: a frame) */
  thumb(abs: string, size: 320 | 640 | 1600, prio: Priority): Promise<Buffer>;
  /** JPEG, long edge ≤ max */
  display(abs: string, max: number): Promise<Buffer>;
  /** JPEG sprite of n frames, h px high */
  strip(abs: string, n: number, h: number): Promise<Buffer>;
}

export const isRemoteAbs = (abs: string) => abs.startsWith('device://');

interface Task {
  run: () => Promise<unknown>;
  resolve: (v: any) => void;
  reject: (e: unknown) => void;
  prio: Priority;
}

/**
 * Concurrency limiter for decode work with two lanes: on-demand requests ('high') always go first,
 * the background thumbnail loop ('low') never takes the last slot.
 */
export class Limiter {
  private running = 0;
  private runningLow = 0;
  private high: Task[] = [];
  private low: Task[] = [];

  constructor(
    readonly max: number,
    readonly lowMax = Math.max(1, max - 1),
  ) {}

  run<T>(fn: () => Promise<T>, prio: Priority): { promise: Promise<T>; promote: () => void } {
    let task!: Task;
    const promise = new Promise<T>((resolve, reject) => {
      task = { run: fn, resolve, reject, prio };
    });
    (prio === 'high' ? this.high : this.low).push(task);
    this.pump();
    return {
      promise,
      promote: () => {
        const i = this.low.indexOf(task);
        if (i >= 0) {
          this.low.splice(i, 1);
          task.prio = 'high';
          this.high.push(task);
          this.pump();
        }
      },
    };
  }

  get stats() {
    return { running: this.running, high: this.high.length, low: this.low.length };
  }

  private pump() {
    while (this.running < this.max) {
      let t: Task | undefined;
      if (this.high.length) t = this.high.shift();
      else if (this.low.length && this.runningLow < this.lowMax) t = this.low.shift();
      if (!t) break;
      const isLow = t.prio === 'low';
      this.running++;
      if (isLow) this.runningLow++;
      void t
        .run()
        .then(t.resolve, t.reject)
        .finally(() => {
          this.running--;
          if (isLow) this.runningLow--;
          this.pump();
        });
    }
  }
}

/**
 * Cache files appear only complete: a concurrent request that sees the file (exists → sendFile,
 * served as immutable) must never get a half written one.
 */
async function writeAtomic(f: string, data: Buffer) {
  const tmp = `${f}.${process.pid}.${crypto.randomBytes(4).toString('hex')}.tmp`;
  try {
    await fsp.writeFile(tmp, data);
    await fsp.rename(tmp, f);
  } catch (e) {
    await fsp.rm(tmp, { force: true });
    throw e;
  }
}

async function toFileAtomic(img: Sharp, f: string) {
  const tmp = `${f}.${process.pid}.${crypto.randomBytes(4).toString('hex')}.tmp`;
  try {
    await img.toFile(tmp);
    await fsp.rename(tmp, f);
  } catch (e) {
    await fsp.rm(tmp, { force: true });
    throw e;
  }
}

async function exists(f: string): Promise<boolean> {
  try {
    await fsp.access(f);
    return true;
  } catch {
    return false;
  }
}

export class ThumbService {
  private inflight = new Map<string, Promise<string | null>>();
  /** one base decode (320 + 640 [+ 1600 for videos] + thumbhash) per source, whatever asked for it */
  private bases = new Map<string, { promise: Promise<BaseResult>; promote: () => void }>();
  readonly limiter = new Limiter(Math.max(2, Math.min(6, os.cpus().length)));
  /** set by the DeviceHub */
  remote: RemoteImageSource | null = null;

  private remoteOf(abs: string): RemoteImageSource | null {
    if (!isRemoteAbs(abs)) return null;
    if (!this.remote) throw new Error('device media unavailable');
    return this.remote;
  }

  constructor(
    private dir: string,
    private ffmpeg: Ffmpeg,
  ) {
    fs.mkdirSync(dir, { recursive: true });
  }

  key(abs: string, row: ThumbRow): string {
    return crypto.createHash('sha1').update(`${abs}|${row.mtime}|${row.size}`).digest('hex');
  }

  file(key: string, variant: string): string {
    return path.join(this.dir, key.slice(0, 2), `${key}-${variant}`);
  }

  private once(id: string, fn: () => Promise<string | null>): Promise<string | null> {
    let p = this.inflight.get(id);
    if (!p) {
      p = fn().finally(() => this.inflight.delete(id));
      this.inflight.set(id, p);
    }
    return p;
  }

  /** Decode any supported still/video into a sharp pipeline (EXIF-rotated). */
  async source(abs: string, row: ThumbRow, maxWidth = 1600): Promise<Sharp | null> {
    if (row.kind === 'video') {
      if (!this.ffmpeg.available) return null;
      const at = row.duration ? Math.min(row.duration * 0.1, 3) : 1;
      const png = await this.ffmpeg.frame(abs, at, maxWidth);
      if (!png.length) return null;
      return sharp(png);
    }
    if (row.kind !== 'image') return null;
    if (row.ext === 'heic' || row.ext === 'heif') {
      const { default: decode } = await import('heic-decode');
      const buf = await fsp.readFile(abs);
      const img = await decode({ buffer: buf });
      return sharp(Buffer.from(img.data.buffer, img.data.byteOffset, img.data.byteLength), { raw: { width: img.width, height: img.height, channels: 4 } });
    }
    return sharp(abs, { failOn: 'none', limitInputPixels: 1e9 }).rotate();
  }

  /**
   * Generate the standard 640 + 320 thumbnails (and 1600 for videos, from the same frame) and a
   * thumbhash from a single decode. Concurrent callers (grid requests of any size, the background
   * loop) share one decode; a 'high' caller promotes a queued background decode.
   */
  generateBase(abs: string, row: ThumbRow, prio: Priority = 'low'): Promise<BaseResult> {
    const key = this.key(abs, row);
    const cur = this.bases.get(key);
    if (cur) {
      if (prio === 'high') cur.promote();
      return cur.promise;
    }
    const remote = this.remoteOf(abs);
    if (remote) {
      // network wait happens outside the decode limiter; only the (cheap) decode takes a slot
      const p = (async () => {
        const f640 = this.file(key, '640.webp');
        const buf = (await exists(f640)) ? await fsp.readFile(f640) : await remote.thumb(abs, 640, prio);
        return this.limiter.run(() => this.decodeBaseFrom(sharp(buf, { failOn: 'none' }).rotate(), key), prio).promise;
      })();
      const entry = { promise: p.finally(() => this.bases.delete(key)), promote: () => {} };
      this.bases.set(key, entry);
      return entry.promise;
    }
    const job = this.limiter.run(() => this.decodeBase(abs, row, key), prio);
    const entry = { promise: job.promise.finally(() => this.bases.delete(key)), promote: job.promote };
    this.bases.set(key, entry);
    return entry.promise;
  }

  private async decodeBase(abs: string, row: ThumbRow, key: string): Promise<BaseResult> {
    const f640 = this.file(key, '640.webp');
    let base: Sharp | null;
    if (row.kind === 'video') {
      // one ffmpeg frame feeds every size (1600 included)
      if (!this.ffmpeg.available) return null;
      const frame = await this.ffmpeg.frame(abs, row.duration ? Math.min(row.duration * 0.1, 3) : 1, 1600);
      if (!frame.length) return null;
      await fsp.mkdir(path.dirname(f640), { recursive: true });
      await toFileAtomic(sharp(frame).resize(1600, 1600, { fit: 'inside', withoutEnlargement: true }).webp({ quality: 85 }), this.file(key, '1600.webp'));
      base = sharp(frame);
    } else {
      base = await this.source(abs, row, 1280);
      if (!base) return null;
    }
    return this.decodeBaseFrom(base, key);
  }

  /** 640 + 320 WebP + thumbhash from a decoded source */
  private async decodeBaseFrom(base: Sharp, key: string): Promise<BaseResult> {
    const f640 = this.file(key, '640.webp');
    const f320 = this.file(key, '320.webp');
    await fsp.mkdir(path.dirname(f640), { recursive: true });
    const { data, info } = await base
      .resize(640, 640, { fit: 'inside', withoutEnlargement: true })
      .webp({ quality: 80 })
      .toBuffer({ resolveWithObject: true });
    // 320 first, 640 last: thumb() treats an existing 640 as "base done"
    await toFileAtomic(sharp(data).resize(320, 320, { fit: 'inside', withoutEnlargement: true }).webp({ quality: 78 }), f320);
    await writeAtomic(f640, data);
    const small = await sharp(data)
      .resize(100, 100, { fit: 'inside' })
      .ensureAlpha()
      .raw()
      .toBuffer({ resolveWithObject: true });
    let hash: string | null = null;
    try {
      hash = Buffer.from(rgbaToThumbHash(small.info.width, small.info.height, small.data)).toString('base64');
    } catch {
      hash = null;
    }
    return { thumbhash: hash, width: info.width, height: info.height };
  }

  async thumb(abs: string, row: ThumbRow, size: ThumbSize, prio: Priority = 'high'): Promise<string | null> {
    const key = this.key(abs, row);
    const f = this.file(key, `${size}.webp`);
    if (await exists(f)) return f;
    const remote = this.remoteOf(abs);
    if (remote && size === 1600) {
      return this.once(f, async () => {
        const buf = await remote.thumb(abs, 1600, prio);
        return this.limiter.run(async () => {
          await fsp.mkdir(path.dirname(f), { recursive: true });
          await toFileAtomic(sharp(buf, { failOn: 'none' }).rotate().resize(1600, 1600, { fit: 'inside', withoutEnlargement: true }).webp({ quality: 85 }), f);
          return f;
        }, prio).promise;
      });
    }
    if (size === 1600 && row.kind !== 'video') {
      return this.once(f, () =>
        this.limiter.run(async () => {
          const src = await this.source(abs, row, 1600);
          if (!src) return null;
          await fsp.mkdir(path.dirname(f), { recursive: true });
          await toFileAtomic(src.resize(1600, 1600, { fit: 'inside', withoutEnlargement: true }).webp({ quality: 85 }), f);
          return f;
        }, prio).promise,
      );
    }
    const r = await this.generateBase(abs, row, prio);
    return r && (await exists(f)) ? f : null;
  }

  /**
   * Browser friendly rendition: full size for HEIC/TIFF/… (max = null), or bounded to `max` px
   * (long edge) for zooming in the viewer without decoding a 50 MP original on a phone.
   * JPEG, or WebP for sources that may carry transparency.
   */
  async display(abs: string, row: ThumbRow, max: number | null = null): Promise<string | null> {
    const key = this.key(abs, row);
    const alpha = ['png', 'webp', 'gif', 'avif', 'svg', 'ico', 'tif', 'tiff'].includes(row.ext);
    const remote = this.remoteOf(abs);
    if (remote) {
      // the phone renders a bounded JPEG (never ships 50 MP originals for display)
      const m = max ?? 4096;
      const rf = this.file(key, `display-${m}.jpg`);
      if (await exists(rf)) return rf;
      return this.once(rf, async () => {
        const buf = await remote.display(abs, m);
        await fsp.mkdir(path.dirname(rf), { recursive: true });
        const tmp = `${rf}.${process.pid}.tmp`;
        await fsp.writeFile(tmp, buf);
        await fsp.rename(tmp, rf);
        return rf;
      });
    }
    const f = this.file(key, max ? `display-${max}.${alpha ? 'webp' : 'jpg'}` : 'full.jpg');
    if (await exists(f)) return f;
    return this.once(f, () =>
      this.limiter.run(async () => {
        const src = await this.source(abs, row, max ?? 4096);
        if (!src) return null;
        await fsp.mkdir(path.dirname(f), { recursive: true });
        const tmp = `${f}.${process.pid}.tmp`;
        const sized = max ? src.resize(max, max, { fit: 'inside', withoutEnlargement: true }) : src;
        if (max && alpha) await sized.webp({ quality: 88 }).toFile(tmp);
        else await sized.flatten({ background: '#ffffff' }).jpeg({ quality: max ? 88 : 90, mozjpeg: true }).toFile(tmp);
        await fsp.rename(tmp, f);
        return f;
      }, 'high').promise,
    );
  }

  /** ~3s muted, small looping preview for hover in the grid. */
  async preview(abs: string, row: ThumbRow): Promise<string | null> {
    if (row.kind !== 'video' || !this.ffmpeg.available || isRemoteAbs(abs)) return null;
    const key = this.key(abs, row);
    const f = this.file(key, 'preview.mp4');
    if (await exists(f)) return f;
    return this.once(f, async () => {
      await fsp.mkdir(path.dirname(f), { recursive: true });
      const dur = row.duration ?? 0;
      const tmp = `${f}.tmp.mp4`;
      let filter: string;
      if (dur > 8) {
        const pts = [0.15, 0.4, 0.65, 0.88].map((p) => dur * p);
        const parts = pts.map(
          (s, i) => `[0:v]trim=start=${s.toFixed(2)}:duration=0.9,setpts=PTS-STARTPTS,scale=-2:240,fps=24[v${i}]`,
        );
        filter = `${parts.join(';')};${pts.map((_, i) => `[v${i}]`).join('')}concat=n=${pts.length}:v=1:a=0,format=yuv420p[out]`;
      } else {
        filter = `[0:v]trim=start=0:duration=3,setpts=PTS-STARTPTS,scale=-2:240,fps=24,format=yuv420p[out]`;
      }
      await this.ffmpeg.run(
        ['-y', '-i', abs, '-filter_complex', filter, '-map', '[out]', '-an', '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '28', '-movflags', '+faststart', tmp],
        { timeoutMs: 120000 },
      );
      await fsp.rename(tmp, f);
      return f;
    });
  }

  /** Horizontal sprite of n frames (video editor timeline). */
  async filmstrip(abs: string, row: ThumbRow, n: number, h: number): Promise<string | null> {
    if (row.kind !== 'video') return null;
    n = Math.max(2, Math.min(60, Math.round(n)));
    h = Math.max(32, Math.min(240, Math.round(h)));
    const key = this.key(abs, row);
    const f = this.file(key, `strip-${n}-${h}.jpg`);
    if (await exists(f)) return f;
    const remote = this.remoteOf(abs);
    if (remote) {
      return this.once(f, async () => {
        const buf = await remote.strip(abs, n, h);
        await fsp.mkdir(path.dirname(f), { recursive: true });
        await fsp.writeFile(`${f}.tmp`, buf);
        await fsp.rename(`${f}.tmp`, f);
        return f;
      });
    }
    if (!this.ffmpeg.available) return null;
    return this.once(f, async () => {
      await fsp.mkdir(path.dirname(f), { recursive: true });
      const dur = Math.max(0.1, row.duration ?? 1);
      const frames: Buffer[] = [];
      for (let i = 0; i < n; i++) {
        const at = (dur * (i + 0.5)) / n;
        const buf = await this.ffmpeg.run(
          ['-ss', at.toFixed(3), '-i', abs, '-frames:v', '1', '-vf', `scale=-2:${h}`, '-f', 'image2pipe', '-vcodec', 'png', 'pipe:1'],
          { timeoutMs: 20000 },
        ).catch(() => Buffer.alloc(0));
        frames.push(buf);
      }
      const valid = frames.filter((b) => b.length);
      if (!valid.length) return null;
      const meta = await sharp(valid[0]).metadata();
      const w = meta.width ?? h;
      const composites = await Promise.all(
        frames.map(async (b, i) => ({ input: b.length ? b : valid[0], left: i * w, top: 0 })),
      );
      await toFileAtomic(sharp({ create: { width: w * n, height: h, channels: 3, background: '#000' } }).composite(composites).jpeg({ quality: 70 }), f);
      return f;
    });
  }

  async clear() {
    await fsp.rm(this.dir, { recursive: true, force: true });
    await fsp.mkdir(this.dir, { recursive: true });
  }

  async sizeBytes(): Promise<number> {
    let total = 0;
    const walk = async (d: string) => {
      for (const e of await fsp.readdir(d, { withFileTypes: true }).catch(() => [])) {
        const p = path.join(d, e.name);
        if (e.isDirectory()) await walk(p);
        else total += (await fsp.stat(p).catch(() => ({ size: 0 }))).size;
      }
    };
    await walk(this.dir);
    return total;
  }
}
