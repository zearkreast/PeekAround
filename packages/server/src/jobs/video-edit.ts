import type { VideoAdjust, VideoEditSpec } from '@peekaround/shared';
import { HttpError } from '../paths.js';

/**
 * Pure planning + ffmpeg argument building for the video editor.
 * No I/O here — the job runner (runners.ts) probes the source, resolves paths
 * and runs the commands this module produces.
 *
 * Filter order: trim (input seek) → crop → rotate → flip → scale → speed →
 * colour (eq, hue, colortemperature) → vignette → overlays → output format.
 * Only filters available in ffmpeg 5.1 (Debian bookworm) are used.
 */

export type VideoFormat = VideoEditSpec['output']['format'];
export type Quality = VideoEditSpec['output']['quality'];

export interface VideoSourceInfo {
  /** displayed size (already swapped for 90/270° rotated sources) */
  width: number;
  height: number;
  duration: number;
  hasAudio: boolean;
  fps: number | null;
}

export interface NormalizedVideoEdit {
  sourceId: string;
  trim: { start: number; end: number } | null;
  crop: { x: number; y: number; w: number; h: number } | null;
  rotate: 0 | 90 | 180 | 270;
  flipH: boolean;
  flipV: boolean;
  speed: number;
  volume: number;
  mute: boolean;
  adjust: VideoAdjust;
  overlays: { assetId: string; start: number; end: number }[];
  output: { format: VideoFormat; height: number | null; quality: Quality };
}

export interface VideoEditPlan extends Omit<NormalizedVideoEdit, 'trim'> {
  /** trim in source seconds (always set) */
  start: number;
  end: number;
  /** true when the input has to be seeked/limited */
  trimmed: boolean;
  /** size after crop + rotate (before output scaling) */
  baseWidth: number;
  baseHeight: number;
  /** final output size (even for mp4/webm) */
  width: number;
  height: number;
  /** expected output duration in seconds */
  duration: number;
  audio: boolean;
  /** output frame rate (gif ≤ 15; never above the source rate after a speed change), null = keep */
  fps: number | null;
  ext: VideoFormat;
}

export const MAX_OVERLAYS = 48;
export const CRF_X264: Record<Quality, number> = { high: 18, medium: 23, low: 28 };
export const CRF_VP9: Record<Quality, number> = { high: 24, medium: 32, low: 40 };
export const CRF_VP8: Record<Quality, number> = { high: 6, medium: 10, low: 20 };
export const GIF_FPS: Record<Quality, number> = { high: 15, medium: 12, low: 10 };
const GIF_COLORS: Record<Quality, number> = { high: 256, medium: 256, low: 128 };
const GIF_DITHER: Record<Quality, string> = { high: 'sierra2_4a', medium: 'bayer:bayer_scale=3', low: 'bayer:bayer_scale=4' };
const AUDIO_BITRATE: Record<Quality, string> = { high: '192k', medium: '160k', low: '128k' };

const bad = (msg: string) => new HttpError(400, msg);

function num(v: unknown, name: string, def?: number): number {
  if (v === undefined || v === null) {
    if (def === undefined) throw bad(`${name} is required`);
    return def;
  }
  const n = typeof v === 'string' && v.trim() !== '' ? Number(v) : v;
  if (typeof n !== 'number' || !Number.isFinite(n)) throw bad(`${name} must be a number`);
  return n;
}

function inRange(n: number, lo: number, hi: number, name: string): number {
  if (n < lo - 1e-9 || n > hi + 1e-9) throw bad(`${name} must be between ${lo} and ${hi}`);
  return Math.min(hi, Math.max(lo, n));
}

/** Shape/range validation of a client spec. Throws HttpError(400). */
export function normalizeVideoEditSpec(spec: unknown): NormalizedVideoEdit {
  if (!spec || typeof spec !== 'object') throw bad('Invalid video edit spec');
  const s = spec as Partial<VideoEditSpec> & Record<string, unknown>;
  if (typeof s.sourceId !== 'string' && typeof s.sourceId !== 'number') throw bad('sourceId is required');
  const sourceId = String(s.sourceId);
  if (!/^\d+$/.test(sourceId)) throw bad('Invalid sourceId');

  let trim: NormalizedVideoEdit['trim'] = null;
  if (s.trim) {
    const start = num(s.trim.start, 'trim.start', 0);
    const end = num(s.trim.end, 'trim.end');
    if (start < 0) throw bad('trim.start must be ≥ 0');
    if (end - start < 0.05) throw bad('trim.end must be after trim.start');
    trim = { start, end };
  }

  let crop: NormalizedVideoEdit['crop'] = null;
  if (s.crop) {
    const x = Math.round(num(s.crop.x, 'crop.x', 0));
    const y = Math.round(num(s.crop.y, 'crop.y', 0));
    const w = Math.round(num(s.crop.w, 'crop.w'));
    const h = Math.round(num(s.crop.h, 'crop.h'));
    if (x < 0 || y < 0) throw bad('crop.x / crop.y must be ≥ 0');
    if (w < 2 || h < 2) throw bad('crop is too small');
    crop = { x, y, w, h };
  }

  const rotate = num(s.rotate, 'rotate', 0);
  if (![0, 90, 180, 270].includes(rotate)) throw bad('rotate must be 0, 90, 180 or 270');

  const speed = inRange(num(s.speed, 'speed', 1), 0.25, 4, 'speed');
  const volume = inRange(num(s.volume, 'volume', 1), 0, 2, 'volume');

  const a = (s.adjust ?? {}) as Partial<VideoAdjust>;
  if (typeof a !== 'object') throw bad('Invalid adjust');
  const adjust: VideoAdjust = {
    brightness: inRange(num(a.brightness, 'adjust.brightness', 0), -1, 1, 'adjust.brightness'),
    contrast: inRange(num(a.contrast, 'adjust.contrast', 0), -1, 1, 'adjust.contrast'),
    saturation: inRange(num(a.saturation, 'adjust.saturation', 0), -1, 1, 'adjust.saturation'),
    hue: inRange(num(a.hue, 'adjust.hue', 0), -180, 180, 'adjust.hue'),
    warmth: inRange(num(a.warmth, 'adjust.warmth', 0), -1, 1, 'adjust.warmth'),
    vignette: inRange(num(a.vignette, 'adjust.vignette', 0), 0, 1, 'adjust.vignette'),
  };

  const overlays: NormalizedVideoEdit['overlays'] = [];
  if (s.overlays !== undefined && s.overlays !== null) {
    if (!Array.isArray(s.overlays)) throw bad('overlays must be an array');
    if (s.overlays.length > MAX_OVERLAYS) throw bad(`At most ${MAX_OVERLAYS} overlays are supported`);
    for (const o of s.overlays) {
      if (!o || typeof o !== 'object' || typeof o.assetId !== 'string') throw bad('Invalid overlay');
      if (!/^[0-9a-f-]{36}\.(png|webp)$/i.test(o.assetId)) throw bad('Overlay assets must be PNG uploads');
      const start = Math.max(0, num(o.start, 'overlay.start', 0));
      const end = num(o.end, 'overlay.end');
      if (end <= start) continue; // never visible
      overlays.push({ assetId: o.assetId, start, end });
    }
  }

  const out = (s.output ?? {}) as Partial<VideoEditSpec['output']>;
  const format = out.format ?? 'mp4';
  if (format !== 'mp4' && format !== 'webm' && format !== 'gif') throw bad('output.format must be mp4, webm or gif');
  const quality = out.quality ?? 'medium';
  if (quality !== 'high' && quality !== 'medium' && quality !== 'low') throw bad('output.quality must be high, medium or low');
  let height: number | null = null;
  if (out.height !== undefined && out.height !== null) {
    height = Math.round(inRange(num(out.height, 'output.height'), 16, 4320, 'output.height'));
  }

  return {
    sourceId,
    trim,
    crop,
    rotate: rotate as NormalizedVideoEdit['rotate'],
    flipH: !!s.flipH,
    flipV: !!s.flipV,
    speed,
    volume,
    mute: !!s.mute,
    adjust,
    overlays,
    output: { format, height, quality },
  };
}

const even = (n: number) => Math.max(2, Math.round(n / 2) * 2);
const evenFloor = (n: number) => Math.max(2, Math.floor(n / 2) * 2);

/** Resolve the spec against the probed source. Throws HttpError(400) for out-of-bounds values. */
export function planVideoEdit(n: NormalizedVideoEdit, src: VideoSourceInfo): VideoEditPlan {
  if (!(src.width > 0 && src.height > 0)) throw bad('The video has no readable video stream');
  if (!(src.duration > 0)) throw bad('Unknown video duration');

  let start = 0;
  let end = src.duration;
  if (n.trim) {
    if (n.trim.start >= src.duration - 0.05) throw bad('trim.start is beyond the end of the video');
    start = n.trim.start;
    end = Math.min(n.trim.end, src.duration);
    if (end - start < 0.05) throw bad('Trimmed clip is too short');
  }
  const trimmed = start > 0.0005 || end < src.duration - 0.0005;

  let crop = n.crop;
  if (crop) {
    if (crop.x >= src.width - 1 || crop.y >= src.height - 1) throw bad('crop is outside the video frame');
    const w = Math.min(crop.w, src.width - crop.x);
    const h = Math.min(crop.h, src.height - crop.y);
    if (w < 2 || h < 2) throw bad('crop is too small');
    crop = { x: crop.x, y: crop.y, w, h };
    if (crop.x === 0 && crop.y === 0 && w === src.width && h === src.height) crop = null;
  }

  let baseWidth = crop ? crop.w : src.width;
  let baseHeight = crop ? crop.h : src.height;
  if (n.rotate === 90 || n.rotate === 270) [baseWidth, baseHeight] = [baseHeight, baseWidth];

  let width: number;
  let height: number;
  if (n.output.height) {
    height = even(n.output.height);
    width = even((baseWidth * height) / baseHeight);
  } else {
    width = evenFloor(baseWidth);
    height = evenFloor(baseHeight);
  }
  if (width > 8192 || height > 8192) throw bad('Output resolution is too large');

  const format = n.output.format;
  const duration = (end - start) / n.speed;
  // overlays that start after the end of the output would never be visible
  const overlays = n.overlays.filter((o) => o.start < duration).map((o) => ({ ...o }));

  // Never exceed the source frame rate: speed-ups select frames (fps filter) instead of
  // producing 2–4× the frame rate, slow-motion keeps every frame at a lower rate.
  let fps: number | null = null;
  const srcFps = src.fps && src.fps > 0 && src.fps < 1000 ? src.fps : null;
  if (srcFps && n.speed !== 1) fps = Math.max(1, Math.round(Math.min(srcFps, srcFps * n.speed) * 1000) / 1000);
  if (format === 'gif') fps = Math.min(GIF_FPS[n.output.quality], fps ?? srcFps ?? Infinity);

  return {
    ...n,
    overlays,
    crop,
    start,
    end,
    trimmed,
    baseWidth,
    baseHeight,
    width,
    height,
    duration,
    audio: src.hasAudio && !n.mute && format !== 'gif',
    fps,
    ext: format,
  };
}

/** Split a speed factor into atempo steps that each stay within 0.5..2. */
export function atempoChain(speed: number): number[] {
  const steps: number[] = [];
  let s = speed;
  while (s > 2 + 1e-9) {
    steps.push(2);
    s /= 2;
  }
  while (s < 0.5 - 1e-9) {
    steps.push(0.5);
    s /= 0.5;
  }
  if (Math.abs(s - 1) > 1e-6) steps.push(s);
  return steps;
}

/** Compact decimal for filter args ("0.5", "2", "0.333333"). */
export function f(n: number, digits = 6): string {
  const s = n.toFixed(digits).replace(/\.?0+$/, '');
  return s === '-0' ? '0' : s;
}

/**
 * Colour adjust mapping (client values → ffmpeg):
 *  brightness b → eq brightness = 0.3·b (additive luma offset)
 *  contrast   c → eq contrast   = 1 + c   (0 = flat grey … 2)
 *  saturation s → eq saturation = 1 + s   (0 = greyscale … 2)
 *  hue        h → hue h = degrees
 *  warmth     w → colortemperature 6500 − 2500·w kelvin (lightness preserved)
 *  vignette   v → vignette angle = v·π/2.5 (0.5 ≈ ffmpeg's default π/5)
 */
export function colorFilters(a: VideoAdjust): string[] {
  const out: string[] = [];
  const eq: string[] = [];
  if (Math.abs(a.brightness) > 1e-4) eq.push(`brightness=${f(a.brightness * 0.3, 4)}`);
  if (Math.abs(a.contrast) > 1e-4) eq.push(`contrast=${f(1 + a.contrast, 4)}`);
  if (Math.abs(a.saturation) > 1e-4) eq.push(`saturation=${f(1 + a.saturation, 4)}`);
  if (eq.length) out.push(`eq=${eq.join(':')}`);
  if (Math.abs(a.hue) > 0.05) out.push(`hue=h=${f(a.hue, 2)}`);
  if (Math.abs(a.warmth) > 1e-3) out.push(`colortemperature=temperature=${Math.round(6500 - 2500 * a.warmth)}:pl=1`);
  if (a.vignette > 1e-3) out.push(`vignette=angle=${f((a.vignette * Math.PI) / 2.5, 4)}`);
  return out;
}

/** The geometric + timing part of the video chain (shared by every output format). */
export function geometryFilters(p: VideoEditPlan): string[] {
  const out: string[] = [];
  if (p.crop) out.push(`crop=${p.crop.w}:${p.crop.h}:${p.crop.x}:${p.crop.y}:exact=1`);
  if (p.rotate === 90) out.push('transpose=clock');
  else if (p.rotate === 270) out.push('transpose=cclock');
  else if (p.rotate === 180) out.push('hflip', 'vflip');
  if (p.flipH) out.push('hflip');
  if (p.flipV) out.push('vflip');
  if (p.width !== p.baseWidth || p.height !== p.baseHeight) out.push(`scale=${p.width}:${p.height}:flags=bicubic`);
  out.push('setsar=1');
  out.push(p.speed === 1 ? 'setpts=PTS-STARTPTS' : `setpts=(PTS-STARTPTS)/${f(p.speed)}`);
  if (p.fps) out.push(`fps=${f(p.fps, 3)}`);
  return out;
}

/**
 * Builds the video filter graph up to (and including) the overlays.
 * Input 0 is the source, inputs 1..n the overlay PNGs. The result ends in `[vo]`.
 */
export function videoGraph(p: VideoEditPlan): string {
  const chain = [...geometryFilters(p), ...colorFilters(p.adjust)];
  const parts: string[] = [];
  let cur = 'vb';
  parts.push(`[0:V:0]${chain.join(',')}[${cur}]`);
  p.overlays.forEach((o, i) => {
    const next = `v${i}`;
    parts.push(`[${i + 1}:v]scale=${p.width}:${p.height},format=rgba[ov${i}]`);
    parts.push(`[${cur}][ov${i}]overlay=0:0:enable='between(t,${f(o.start, 3)},${f(o.end, 3)})'[${next}]`);
    cur = next;
  });
  parts.push(`[${cur}]null[vo]`);
  return parts.join(';');
}

export function audioGraph(p: VideoEditPlan): string | null {
  if (!p.audio) return null;
  const chain = ['asetpts=PTS-STARTPTS', ...atempoChain(p.speed).map((s) => `atempo=${f(s)}`)];
  if (Math.abs(p.volume - 1) > 1e-4) chain.push(`volume=${f(p.volume, 4)}`);
  return `[0:a:0]${chain.join(',')}[ao]`;
}

export interface VideoEditIO {
  input: string;
  overlays: string[];
  output: string;
  /** gif only: temp palette file */
  palette?: string;
  /** encoder names available in this ffmpeg build */
  encoders: Set<string>;
}

export interface FfmpegPass {
  args: string[];
  /** share of the job's total progress */
  weight: number;
}

function inputArgs(p: VideoEditPlan, io: VideoEditIO): string[] {
  const a: string[] = [];
  if (p.trimmed) a.push('-ss', f(p.start, 3), '-t', f(p.end - p.start, 3));
  a.push('-i', io.input);
  for (const o of io.overlays) a.push('-i', o);
  return a;
}

/** Choose the encoders for a format, or throw 400 when this ffmpeg can't produce it. */
export function pickCodecs(format: VideoFormat, encoders: Set<string>): { video: string; audio: string | null } {
  if (format === 'gif') return { video: 'gif', audio: null };
  if (format === 'mp4') {
    if (!encoders.has('libx264')) throw bad('This ffmpeg build has no H.264 encoder (libx264)');
    return { video: 'libx264', audio: 'aac' };
  }
  const video = encoders.has('libvpx-vp9') ? 'libvpx-vp9' : encoders.has('libvpx') ? 'libvpx' : null;
  if (!video) throw bad('This ffmpeg build cannot encode WebM (libvpx missing)');
  const audio = encoders.has('libopus') ? 'libopus' : encoders.has('libvorbis') ? 'libvorbis' : null;
  return { video, audio };
}

/** All ffmpeg invocations for a plan (gif = palette pass + encode pass). */
export function buildVideoEditPasses(p: VideoEditPlan, io: VideoEditIO): FfmpegPass[] {
  if (io.overlays.length !== p.overlays.length) throw new Error('overlay paths do not match the plan');
  const codecs = pickCodecs(p.ext, io.encoders);
  const q = p.output.quality;
  const vgraph = videoGraph(p);

  if (p.ext === 'gif') {
    if (!io.palette) throw new Error('palette path required for gif output');
    const palIdx = 1 + io.overlays.length;
    const palette: string[] = [
      '-y',
      ...inputArgs(p, io),
      '-filter_complex',
      `${vgraph};[vo]palettegen=stats_mode=diff:max_colors=${GIF_COLORS[q]}[pal]`,
      '-map',
      '[pal]',
      '-frames:v',
      '1',
      '-update',
      '1',
      '-f',
      'image2',
      io.palette,
    ];
    const encode: string[] = [
      '-y',
      ...inputArgs(p, io),
      '-i',
      io.palette,
      '-filter_complex',
      `${vgraph};[vo][${palIdx}:v]paletteuse=dither=${GIF_DITHER[q]}:diff_mode=rectangle[g]`,
      '-map',
      '[g]',
      '-an',
      '-map_chapters',
      '-1',
      '-c:v',
      'gif',
      '-loop',
      '0',
      '-f',
      'gif',
      io.output,
    ];
    return [
      { args: palette, weight: 0.35 },
      { args: encode, weight: 0.65 },
    ];
  }

  const agraph = codecs.audio ? audioGraph(p) : null;
  const args: string[] = ['-y', ...inputArgs(p, io), '-filter_complex', agraph ? `${vgraph};${agraph}` : vgraph, '-map', '[vo]'];
  if (agraph) args.push('-map', '[ao]');
  else args.push('-an');
  args.push('-map_chapters', '-1', '-sn', '-dn');

  if (codecs.video === 'libx264') {
    args.push('-c:v', 'libx264', '-preset', 'medium', '-crf', String(CRF_X264[q]), '-pix_fmt', 'yuv420p');
  } else if (codecs.video === 'libvpx-vp9') {
    args.push('-c:v', 'libvpx-vp9', '-crf', String(CRF_VP9[q]), '-b:v', '0', '-deadline', 'good', '-cpu-used', '4', '-row-mt', '1', '-pix_fmt', 'yuv420p');
  } else {
    args.push('-c:v', 'libvpx', '-crf', String(CRF_VP8[q]), '-b:v', '8M', '-deadline', 'good', '-cpu-used', '4', '-pix_fmt', 'yuv420p');
  }
  if (agraph && codecs.audio) args.push('-c:a', codecs.audio, '-b:a', AUDIO_BITRATE[q]);
  args.push('-max_muxing_queue_size', '4096');
  if (p.ext === 'mp4') args.push('-movflags', '+faststart', '-f', 'mp4');
  else args.push('-f', 'webm');
  args.push(io.output);
  return [{ args, weight: 1 }];
}
