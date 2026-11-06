import os from 'node:os';
import sharp from 'sharp';
import type { IndexIntensity, Settings, ThumbQuality } from '@peekaround/shared';
import type { Ctx } from './context.js';

/**
 * Settings that tune running services (thumbnail quality, indexing concurrency, ffmpeg hwaccel).
 * Validated here (PATCH /api/settings) and applied on start + every save (context.saveSettings).
 */

const QUALITY: ThumbQuality[] = ['low', 'normal', 'high'];
const INTENSITY: IndexIntensity[] = ['low', 'normal', 'high'];

/** copy the valid tuning keys of a PATCH body into `patch` (invalid values are ignored like the other keys) */
export function pickRuntimeSettings(b: Partial<Settings>, patch: Partial<Settings>) {
  if (typeof b.thumbQuality === 'string' && QUALITY.includes(b.thumbQuality)) patch.thumbQuality = b.thumbQuality;
  if (typeof b.indexIntensity === 'string' && INTENSITY.includes(b.indexIntensity)) patch.indexIntensity = b.indexIntensity;
  if (typeof b.hwAccel === 'boolean') patch.hwAccel = b.hwAccel;
}

export interface IntensityLimits {
  /** thumbnail limiter: decodes at once (on-demand + background) */
  max: number;
  /** of which background decodes */
  lowMax: number;
  /** background items at once across every pass (meta, thumbnails, enrichment, hashing) */
  workers: number;
  /** pause after each background item (ms) */
  gapMs: number;
  /** libvips threads per image operation (sharp.concurrency); 0 = one per core */
  vipsThreads: number;
  /** ffmpeg decoder threads for thumbnail frames / probes; 0 = ffmpeg's default (one per core) */
  ffmpegThreads: number;
  /** @deprecated same as workers (kept for older callers) */
  meta: number;
  /** @deprecated same as workers */
  thumb: number;
}

/**
 * Background CPU budget per indexing intensity. libvips (sharp) and ffmpeg start one thread per core
 * for every image / video by default, so a few parallel decodes would fill every core; each
 * background decode gets a single thread instead and the number of parallel items is the budget:
 *   low    → 1 item at a time, 1 thread, short pause after each item (≈ one core at most)
 *   normal → about half of the cores on a server (at most 3), a quarter in the desktop app, where
 *            the server shares Electron's main process with the window (at most 2)
 *   high   → all but one core, libvips/ffmpeg threading left on (fastest)
 */
export function intensityLimits(i: IndexIntensity, cpus = os.cpus().length, desktop = false): IntensityLimits {
  const n = Math.max(1, cpus);
  const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, Math.floor(v)));
  let r: Omit<IntensityLimits, 'meta' | 'thumb'>;
  if (i === 'low') r = { max: clamp(n / 2, 2, 4), lowMax: 1, workers: 1, gapMs: 40, vipsThreads: 1, ffmpegThreads: 1 };
  else if (i === 'high') {
    const workers = clamp(n - 1, 2, 8);
    r = { max: clamp(n, 2, 12), lowMax: workers, workers, gapMs: 0, vipsThreads: 0, ffmpegThreads: 0 };
  } else {
    const workers = desktop ? clamp(n / 4, 1, 2) : clamp(n / 2, 1, 3);
    r = { max: clamp(n, 2, 6), lowMax: workers, workers, gapMs: 0, vipsThreads: 1, ffmpegThreads: 2 };
  }
  return { ...r, lowMax: Math.min(r.lowMax, r.max), meta: r.workers, thumb: r.workers };
}

export function applyRuntimeSettings(ctx: Pick<Ctx, 'thumbs' | 'scanner' | 'ffmpeg' | 'cfg'>, s: Settings) {
  const lim = intensityLimits(s.indexIntensity ?? 'normal', os.cpus().length, ctx.cfg?.mode === 'desktop');
  ctx.thumbs.quality = s.thumbQuality ?? 'normal';
  ctx.thumbs.limiter.resize(lim.max, lim.lowMax);
  ctx.thumbs.bg.configure(lim.workers, lim.gapMs);
  ctx.scanner.concurrency = { meta: lim.workers, thumb: lim.workers };
  // libvips worker threads per pipeline (process wide; 0 = libvips default: one per core)
  sharp.concurrency(lim.vipsThreads || os.cpus().length);
  ctx.ffmpeg.decodeThreads = lim.ffmpegThreads;
}

/** ffmpeg input options for GPU decoding (before -i); frames come back to system memory for libx264 */
export function hwAccelArgs(s: Pick<Settings, 'hwAccel'>): string[] {
  return s.hwAccel ? ['-hwaccel', 'auto'] : [];
}
