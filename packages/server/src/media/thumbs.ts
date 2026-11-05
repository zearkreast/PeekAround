import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
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

export class ThumbService {
  private inflight = new Map<string, Promise<string | null>>();

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
   * Generate the standard 640 + 320 thumbnails and a thumbhash from a single decode.
   * Returns thumbhash (base64) and real dimensions when they were unknown.
   */
  async generateBase(abs: string, row: ThumbRow): Promise<{ thumbhash: string | null; width: number; height: number } | null> {
    const key = this.key(abs, row);
    const f640 = this.file(key, '640.webp');
    const f320 = this.file(key, '320.webp');
    const src = await this.source(abs, row, 1280);
    if (!src) return null;
    await fsp.mkdir(path.dirname(f640), { recursive: true });
    const { data, info } = await src
      .resize(640, 640, { fit: 'inside', withoutEnlargement: true })
      .webp({ quality: 80 })
      .toBuffer({ resolveWithObject: true });
    await fsp.writeFile(f640, data);
    await sharp(data).resize(320, 320, { fit: 'inside', withoutEnlargement: true }).webp({ quality: 78 }).toFile(f320);
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

  async thumb(abs: string, row: ThumbRow, size: ThumbSize): Promise<string | null> {
    const key = this.key(abs, row);
    const f = this.file(key, `${size}.webp`);
    if (fs.existsSync(f)) return f;
    return this.once(f, async () => {
      if (size === 1600) {
        const src = await this.source(abs, row, 1600);
        if (!src) return null;
        await fsp.mkdir(path.dirname(f), { recursive: true });
        await src.resize(1600, 1600, { fit: 'inside', withoutEnlargement: true }).webp({ quality: 85 }).toFile(f);
        return f;
      }
      const r = await this.generateBase(abs, row);
      return r && fs.existsSync(f) ? f : null;
    });
  }

  /** Full resolution browser-friendly version of images the browser can't decode (HEIC, TIFF…). */
  async display(abs: string, row: ThumbRow): Promise<string | null> {
    const key = this.key(abs, row);
    const f = this.file(key, 'full.jpg');
    if (fs.existsSync(f)) return f;
    return this.once(f, async () => {
      const src = await this.source(abs, row, 4096);
      if (!src) return null;
      await fsp.mkdir(path.dirname(f), { recursive: true });
      await src.jpeg({ quality: 90, mozjpeg: true }).toFile(f);
      return f;
    });
  }

  /** ~3s muted, small looping preview for hover in the grid. */
  async preview(abs: string, row: ThumbRow): Promise<string | null> {
    if (row.kind !== 'video' || !this.ffmpeg.available) return null;
    const key = this.key(abs, row);
    const f = this.file(key, 'preview.mp4');
    if (fs.existsSync(f)) return f;
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
    if (row.kind !== 'video' || !this.ffmpeg.available) return null;
    n = Math.max(2, Math.min(60, Math.round(n)));
    h = Math.max(32, Math.min(240, Math.round(h)));
    const key = this.key(abs, row);
    const f = this.file(key, `strip-${n}-${h}.jpg`);
    if (fs.existsSync(f)) return f;
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
      await sharp({ create: { width: w * n, height: h, channels: 3, background: '#000' } })
        .composite(composites)
        .jpeg({ quality: 70 })
        .toFile(f);
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
