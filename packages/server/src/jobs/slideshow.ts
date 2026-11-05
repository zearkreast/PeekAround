import type { SlideshowExportSpec, SlideTransition } from '@peekaround/shared';
import { HttpError, sanitizeName } from '../paths.js';
import { f } from './video-edit.js';

/**
 * Pure planning + ffmpeg argument building for slideshow → MP4 export.
 *
 * Rendering model (keeps memory bounded for any number of slides):
 *  - every image is pre-rendered by the runner (sharp, EXIF-rotated, fit + padded
 *    with the background colour) to a raw RGB "canvas" file;
 *  - the timeline is cut into one *piece* per slide: slide i from the end of the
 *    previous transition up to its own end, cross-fading (xfade) into the head of
 *    slide i+1 over its last `cuts[i].frames` frames;
 *  - every piece is rendered by its own short ffmpeg (≤ 2 inputs) as raw yuv420p
 *    frames, which the runner streams into a single long-running x264 encoder that
 *    also muxes the (looped, trimmed, faded) music.
 * All timing is done in whole frames, so the output duration is exact.
 */

export type XfadeName = string;

/** Our transition names → ffmpeg xfade transitions (all present in ffmpeg ≥ 5.0). */
export const XFADE_TRANSITIONS: Record<Exclude<SlideTransition, 'none' | 'random'>, XfadeName> = {
  fade: 'fade',
  slide: 'slideleft',
  zoom: 'zoomin',
  blur: 'hblur',
  flip: 'squeezev',
  circle: 'circleopen',
  kenburns: 'fade',
};

/** Pool for `random` (picked per cut, never the same twice in a row). */
export const RANDOM_TRANSITIONS: XfadeName[] = [
  'fade',
  'slideleft',
  'slideright',
  'slideup',
  'slidedown',
  'circleopen',
  'circleclose',
  'zoomin',
  'hblur',
  'squeezev',
  'smoothleft',
  'smoothright',
  'radial',
  'fadeblack',
  'wipeleft',
  'diagtl',
];

export const SLIDE_TRANSITIONS: SlideTransition[] = ['none', 'fade', 'slide', 'zoom', 'kenburns', 'blur', 'flip', 'circle', 'random'];

export const MAX_ITEMS = 1000;
export const MAX_DURATION = 3 * 3600;
/**
 * Ken Burns frames are cropped (zoompan) from a canvas rendered up to 2× larger than the
 * output, in yuv444p (no chroma alignment), so pan/zoom steps are ≤ ½ output pixel.
 */
export const MAX_KB_CANVAS = 7680;
export const MAX_KB_SCALE = 2;
export const SLIDESHOW_CRF = 20;

const bad = (msg: string) => new HttpError(400, msg);

export interface NormalizedSlideshow {
  items: { id: string; duration: number | null; trim: { start: number; end: number } | null }[];
  duration: number;
  transition: SlideTransition;
  transitionDuration: number;
  kenBurns: boolean;
  audioId: string | null;
  width: number;
  height: number;
  fps: number;
  /** '#rrggbb' */
  background: string;
  target: { folderId: string; dir: string } | null;
  /** file name without extension (sanitized) */
  name: string;
}

function num(v: unknown, name: string, def?: number): number {
  if (v === undefined || v === null || v === '') {
    if (def === undefined) throw bad(`${name} is required`);
    return def;
  }
  const n = typeof v === 'string' ? Number(v) : v;
  if (typeof n !== 'number' || !Number.isFinite(n)) throw bad(`${name} must be a number`);
  return n;
}

function range(n: number, lo: number, hi: number, name: string): number {
  if (n < lo - 1e-9 || n > hi + 1e-9) throw bad(`${name} must be between ${lo} and ${hi}`);
  return Math.min(hi, Math.max(lo, n));
}

function mediaId(v: unknown, name: string): string {
  if (typeof v !== 'string' && typeof v !== 'number') throw bad(`${name} is required`);
  const s = String(v);
  if (!/^\d+$/.test(s)) throw bad(`Invalid ${name}`);
  return s;
}

/** '#abc' | 'abc' | '#aabbcc' → '#aabbcc' */
export function normalizeHex(v: unknown): string {
  if (v === undefined || v === null || v === '') return '#000000';
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(String(v).trim());
  if (!m) throw bad('background must be a hex colour like #000000');
  let h = m[1].toLowerCase();
  if (h.length === 3) h = h.replace(/./g, (c) => c + c);
  return `#${h}`;
}

/** Shape/range validation of a client spec. Throws HttpError(400). */
export function normalizeSlideshowSpec(spec: unknown): NormalizedSlideshow {
  if (!spec || typeof spec !== 'object') throw bad('Invalid slideshow spec');
  const s = spec as Partial<SlideshowExportSpec> & Record<string, unknown>;
  if (!Array.isArray(s.items) || !s.items.length) throw bad('items must contain at least one media id');
  if (s.items.length > MAX_ITEMS) throw bad(`At most ${MAX_ITEMS} items are supported`);
  const items = s.items.map((it, i) => {
    if (!it || typeof it !== 'object') throw bad(`Invalid item ${i + 1}`);
    const id = mediaId(it.id, `items[${i}].id`);
    const duration = it.duration === undefined || it.duration === null ? null : range(num(it.duration, `items[${i}].duration`), 0.2, 3600, `items[${i}].duration`);
    let trim: { start: number; end: number } | null = null;
    if (it.trim) {
      const start = Math.max(0, num(it.trim.start, `items[${i}].trim.start`, 0));
      const end = num(it.trim.end, `items[${i}].trim.end`);
      if (end - start < 0.05) throw bad(`items[${i}].trim.end must be after trim.start`);
      trim = { start, end };
    }
    return { id, duration, trim };
  });

  const transition = (s.transition ?? 'fade') as SlideTransition;
  if (!SLIDE_TRANSITIONS.includes(transition)) throw bad(`Unknown transition "${String(s.transition)}"`);

  const even = (n: number) => Math.round(n / 2) * 2;
  const width = even(range(Math.round(num(s.width, 'width', 1920)), 64, 4096, 'width'));
  const height = even(range(Math.round(num(s.height, 'height', 1080)), 64, 4096, 'height'));

  let target: NormalizedSlideshow['target'] = null;
  if (s.target) {
    if (typeof s.target !== 'object') throw bad('Invalid target');
    target = { folderId: mediaId(s.target.folderId, 'target.folderId'), dir: typeof s.target.dir === 'string' ? s.target.dir : '' };
  }

  let name = typeof s.name === 'string' ? s.name.trim().replace(/\.mp4$/i, '') : '';
  if (name.length > 150) name = name.slice(0, 150);
  name = name ? sanitizeName(name) : 'Slayt';

  return {
    items,
    duration: range(num(s.duration, 'duration', 4), 0.2, 600, 'duration'),
    transition,
    transitionDuration: range(num(s.transitionDuration, 'transitionDuration', 0.8), 0, 10, 'transitionDuration'),
    kenBurns: !!s.kenBurns,
    audioId: s.audioId === undefined || s.audioId === null || s.audioId === '' ? null : mediaId(s.audioId, 'audioId'),
    width,
    height,
    fps: Math.round(range(num(s.fps, 'fps', 30), 1, 60, 'fps')),
    background: normalizeHex(s.background),
    target,
    name,
  };
}

/** What the runner knows about each item before planning. */
export type SlideSource = { kind: 'image' } | { kind: 'video'; duration: number };

export interface KenBurnsMove {
  zoomFrom: number;
  zoomTo: number;
  /** focus point as a fraction of the free margin (0 = left/top, 0.5 = centre, 1 = right/bottom) */
  xFrom: number;
  xTo: number;
  yFrom: number;
  yTo: number;
}

/** Alternating zoom-in / zoom-out with varying pan directions. */
export const KEN_BURNS_MOVES: KenBurnsMove[] = [
  { zoomFrom: 1, zoomTo: 1.15, xFrom: 0.35, xTo: 0.65, yFrom: 0.5, yTo: 0.5 }, // in, drift right
  { zoomFrom: 1.15, zoomTo: 1, xFrom: 0.65, xTo: 0.35, yFrom: 0.4, yTo: 0.6 }, // out, drift left
  { zoomFrom: 1, zoomTo: 1.15, xFrom: 0.5, xTo: 0.5, yFrom: 0.65, yTo: 0.35 }, // in, drift up
  { zoomFrom: 1.15, zoomTo: 1, xFrom: 0.35, xTo: 0.65, yFrom: 0.35, yTo: 0.65 }, // out, diagonal
];

export interface SlidePlan {
  index: number;
  kind: 'image' | 'video';
  /** length in frames, including the parts shared with transitions */
  frames: number;
  /** video only: source second where the clip starts */
  videoStart: number;
  kenBurns: KenBurnsMove | null;
}

export interface CutPlan {
  /** overlap in frames (0 = hard cut) */
  frames: number;
  transition: XfadeName | null;
}

export interface SlideshowPlan {
  width: number;
  height: number;
  fps: number;
  background: string;
  /** Ken Burns canvas scale factor */
  kbScale: number;
  slides: SlidePlan[];
  /** cuts[i] is between slides i and i+1 */
  cuts: CutPlan[];
  totalFrames: number;
  /** seconds */
  duration: number;
}

export function planSlideshow(n: NormalizedSlideshow, sources: SlideSource[], rnd: () => number = Math.random): SlideshowPlan {
  if (sources.length !== n.items.length) throw new Error('sources do not match items');
  const F = n.fps;
  const kenBurns = n.kenBurns || n.transition === 'kenburns';
  let kbCount = 0;
  const slides: SlidePlan[] = n.items.map((it, i) => {
    const src = sources[i];
    if (src.kind === 'image') {
      const secs = it.duration ?? n.duration;
      return {
        index: i,
        kind: 'image',
        frames: Math.max(1, Math.round(secs * F)),
        videoStart: 0,
        kenBurns: kenBurns ? KEN_BURNS_MOVES[kbCount++ % KEN_BURNS_MOVES.length] : null,
      };
    }
    const dur = src.duration;
    if (!(dur > 0)) throw bad(`Item ${i + 1}: unknown video duration`);
    let start = 0;
    let end = dur;
    if (it.trim) {
      start = Math.min(it.trim.start, dur);
      end = Math.min(it.trim.end, dur);
    } else if (it.duration) end = Math.min(dur, it.duration);
    const frames = Math.round((end - start) * F);
    if (frames < 1) throw bad(`Item ${i + 1}: the video clip is too short`);
    return { index: i, kind: 'video', frames, videoStart: start, kenBurns: null };
  });

  const cuts: CutPlan[] = [];
  let prev: string | null = null;
  for (let i = 0; i + 1 < slides.length; i++) {
    const want = n.transition === 'none' ? 0 : Math.round(n.transitionDuration * F);
    // each slide may give at most half of itself to each neighbouring transition
    const frames = Math.max(0, Math.min(want, Math.floor(slides[i].frames / 2), Math.floor(slides[i + 1].frames / 2)));
    let transition: string | null = null;
    if (frames > 0) {
      if (n.transition === 'random') {
        const pool = RANDOM_TRANSITIONS.filter((t) => t !== prev);
        transition = pool[Math.min(pool.length - 1, Math.floor(rnd() * pool.length))];
      } else transition = XFADE_TRANSITIONS[n.transition as keyof typeof XFADE_TRANSITIONS];
      prev = transition;
    }
    cuts.push({ frames, transition: frames > 0 ? transition : null });
  }

  const totalFrames = slides.reduce((a, s) => a + s.frames, 0) - cuts.reduce((a, c) => a + c.frames, 0);
  const duration = totalFrames / F;
  if (duration > MAX_DURATION) throw bad('The slideshow is too long (max 3 hours)');
  const kbScale = kenBurns ? Math.max(1, Math.min(MAX_KB_SCALE, Math.floor(MAX_KB_CANVAS / Math.max(n.width, n.height)))) : 1;
  return { width: n.width, height: n.height, fps: F, background: n.background, kbScale, slides, cuts, totalFrames, duration };
}

/** Size of the pre-rendered RGB canvas of an image slide. */
export function canvasSize(p: SlideshowPlan, s: SlidePlan): { width: number; height: number } {
  const k = s.kenBurns ? p.kbScale : 1;
  return { width: p.width * k, height: p.height * k };
}

export interface PieceRange {
  slide: number;
  /** first local frame of slide i rendered by this piece */
  from: number;
  /** frames this piece contributes to the output */
  frames: number;
  /** the cross-fade into the next slide at the end of this piece, if any */
  xfade: { transition: XfadeName; frames: number } | null;
}

export function pieceRange(p: SlideshowPlan, i: number): PieceRange {
  const s = p.slides[i];
  const from = i > 0 ? p.cuts[i - 1].frames : 0;
  const cut = i < p.cuts.length ? p.cuts[i] : null;
  return {
    slide: i,
    from,
    frames: s.frames - from,
    xfade: cut && cut.frames > 0 && cut.transition ? { transition: cut.transition, frames: cut.frames } : null,
  };
}

export type PieceInput = { kind: 'canvas'; path: string; width: number; height: number } | { kind: 'video'; path: string };

const BT709 = 'scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setsar=1';

/** zoompan expressions for a move, rendering local frames from..from+count of a slide of `total` frames. */
export function kenBurnsExpr(m: KenBurnsMove, from: number, total: number): { z: string; x: string; y: string } {
  const den = Math.max(1, total - 1);
  const P = `((on+${from})/${den})`;
  const lerp = (a: number, b: number) => (Math.abs(b - a) < 1e-9 ? f(a, 4) : `(${f(a, 4)}+${f(b - a, 4)}*${P})`);
  return {
    z: lerp(m.zoomFrom, m.zoomTo),
    x: `(iw-iw/zoom)*${lerp(m.xFrom, m.xTo)}`,
    y: `(ih-ih/zoom)*${lerp(m.yFrom, m.yTo)}`,
  };
}

function slideBranch(
  p: SlideshowPlan,
  s: SlidePlan,
  input: PieceInput,
  idx: number,
  from: number,
  count: number,
  label: string,
): { args: string[]; filter: string } {
  const F = p.fps;
  const W = p.width;
  const H = p.height;
  if (s.kind === 'video') {
    if (input.kind !== 'video') throw new Error('video slide needs a video input');
    const ss = s.videoStart + from / F;
    const bg = `0x${p.background.slice(1)}`;
    return {
      args: [...(ss > 0.0005 ? ['-ss', f(ss, 3)] : []), '-i', input.path],
      filter:
        `[${idx}:V:0]setpts=PTS-STARTPTS,scale=${W}:${H}:force_original_aspect_ratio=decrease:force_divisible_by=2,` +
        `pad=${W}:${H}:(ow-iw)/2:(oh-ih)/2:color=${bg},setsar=1,fps=${F},format=yuv420p,` +
        `tpad=stop_mode=clone:stop=-1,trim=end_frame=${count}[${label}]`,
    };
  }
  if (input.kind !== 'canvas') throw new Error('image slide needs a canvas input');
  const args = ['-f', 'rawvideo', '-pix_fmt', 'rgb24', '-video_size', `${input.width}x${input.height}`, '-framerate', String(F), '-i', input.path];
  if (s.kenBurns) {
    const e = kenBurnsExpr(s.kenBurns, from, s.frames);
    return {
      args,
      filter:
        `[${idx}:v]scale=out_color_matrix=bt709:out_range=tv,format=yuv444p,` +
        `zoompan=z='${e.z}':x='${e.x}':y='${e.y}':d=${count}:s=${W}x${H}:fps=${F},format=yuv420p,setsar=1[${label}]`,
    };
  }
  return {
    args,
    filter: `[${idx}:v]${BT709},tpad=stop_mode=clone:stop=-1,trim=end_frame=${count}[${label}]`,
  };
}

/**
 * ffmpeg args rendering piece `i` as raw yuv420p frames on stdout.
 * `inputs[0]` is slide i, `inputs[1]` slide i+1 (only needed when the piece ends in a transition).
 */
export function buildPieceArgs(p: SlideshowPlan, i: number, inputs: PieceInput[]): { args: string[]; frames: number } {
  const r = pieceRange(p, i);
  const a = slideBranch(p, p.slides[i], inputs[0], 0, r.from, r.frames, r.xfade ? 'a' : 'v');
  const args = ['-y', ...a.args];
  let graph = a.filter;
  if (r.xfade) {
    if (!inputs[1]) throw new Error('transition piece needs the next slide');
    const b = slideBranch(p, p.slides[i + 1], inputs[1], 1, 0, r.xfade.frames, 'b');
    args.push(...b.args);
    const dur = r.xfade.frames / p.fps;
    const off = (r.frames - r.xfade.frames) / p.fps;
    graph += `;${b.filter};[a][b]xfade=transition=${r.xfade.transition}:duration=${f(dur)}:offset=${f(off)}[v]`;
  }
  args.push(
    '-filter_complex',
    graph,
    '-map',
    '[v]',
    '-frames:v',
    String(r.frames),
    '-an',
    '-sn',
    '-dn',
    '-fps_mode',
    'passthrough',
    '-f',
    'rawvideo',
    '-pix_fmt',
    'yuv420p',
    'pipe:1',
  );
  return { args, frames: r.frames };
}

export function frameBytes(p: Pick<SlideshowPlan, 'width' | 'height'>): number {
  return (p.width * p.height * 3) / 2;
}

/** The final encoder: raw frames on stdin (+ optional looped music) → MP4. */
export function buildEncoderArgs(p: SlideshowPlan, io: { output: string; music?: string | null }): string[] {
  const D = p.duration;
  const args = [
    '-y',
    '-f',
    'rawvideo',
    '-pix_fmt',
    'yuv420p',
    '-video_size',
    `${p.width}x${p.height}`,
    '-framerate',
    String(p.fps),
    '-i',
    'pipe:0',
  ];
  if (io.music) {
    const fade = Math.min(2, D);
    args.push('-stream_loop', '-1', '-i', io.music);
    args.push(
      '-filter_complex',
      `[1:a:0]atrim=end=${f(D)},asetpts=PTS-STARTPTS,afade=t=out:st=${f(Math.max(0, D - fade))}:d=${f(fade)}[a]`,
      '-map',
      '0:v',
      '-map',
      '[a]',
    );
  } else args.push('-map', '0:v');
  args.push(
    '-c:v',
    'libx264',
    '-preset',
    'medium',
    '-crf',
    String(SLIDESHOW_CRF),
    '-pix_fmt',
    'yuv420p',
    '-color_primaries',
    'bt709',
    '-color_trc',
    'bt709',
    '-colorspace',
    'bt709',
    '-color_range',
    'tv',
  );
  if (io.music) args.push('-c:a', 'aac', '-b:a', '192k');
  args.push('-t', f(D), '-max_muxing_queue_size', '4096', '-movflags', '+faststart', '-f', 'mp4', io.output);
  return args;
}

/** One solid frame (BT.709 limited range yuv420p) used to pad pieces that came up short. */
export function solidFrame(width: number, height: number, hex: string): Buffer {
  const h = normalizeHex(hex).slice(1);
  const r = parseInt(h.slice(0, 2), 16) / 255;
  const g = parseInt(h.slice(2, 4), 16) / 255;
  const b = parseInt(h.slice(4, 6), 16) / 255;
  const y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  const clamp = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
  const Y = clamp(16 + 219 * y);
  const U = clamp(128 + (224 * (b - y)) / 1.8556);
  const V = clamp(128 + (224 * (r - y)) / 1.5748);
  const luma = width * height;
  const chroma = luma / 4;
  const buf = Buffer.alloc(luma + 2 * chroma);
  buf.fill(Y, 0, luma);
  buf.fill(U, luma, luma + chroma);
  buf.fill(V, luma + chroma);
  return buf;
}
