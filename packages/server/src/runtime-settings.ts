import os from 'node:os';
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

/** decode concurrency per indexing intensity: [thumbnail limiter max, its background lane, meta pass, thumb pass] */
export function intensityLimits(i: IndexIntensity, cpus = os.cpus().length) {
  const base = Math.max(2, Math.min(6, cpus));
  if (i === 'low') return { max: base, lowMax: 1, meta: 1, thumb: 1 };
  if (i === 'high') {
    const max = Math.max(2, Math.min(12, cpus));
    return { max, lowMax: Math.max(1, max - 1), meta: max, thumb: Math.max(1, Math.min(8, cpus - 1)) };
  }
  return { max: base, lowMax: Math.max(1, base - 1), meta: base, thumb: Math.max(1, Math.min(4, cpus - 1)) };
}

export function applyRuntimeSettings(ctx: Pick<Ctx, 'thumbs' | 'scanner'>, s: Settings) {
  const lim = intensityLimits(s.indexIntensity ?? 'normal');
  ctx.thumbs.quality = s.thumbQuality ?? 'normal';
  ctx.thumbs.limiter.resize(lim.max, lim.lowMax);
  ctx.scanner.concurrency = { meta: lim.meta, thumb: lim.thumb };
}

/** ffmpeg input options for GPU decoding (before -i); frames come back to system memory for libx264 */
export function hwAccelArgs(s: Pick<Settings, 'hwAccel'>): string[] {
  return s.hwAccel ? ['-hwaccel', 'auto'] : [];
}
