import type { VideoAdjust, VideoEditSpec, VideoOverlay } from '@peekaround/shared';
import type { TextLayer } from '@/lib/text';

/**
 * Pure model of the video editor. Geometry rules mirror the server planner
 * (packages/server/src/jobs/video-edit.ts → planVideoEdit) so overlays rendered here
 * line up exactly with the exported video. Keep the two in sync.
 */

export type Rotation = 0 | 90 | 180 | 270;
export type Format = VideoEditSpec['output']['format'];
export type Quality = VideoEditSpec['output']['quality'];
export type AspectKey = 'free' | '16:9' | '9:16' | '1:1' | '4:5';

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Size {
  w: number;
  h: number;
}

export const ASPECT_RATIOS: Record<Exclude<AspectKey, 'free'>, number> = {
  '16:9': 16 / 9,
  '9:16': 9 / 16,
  '1:1': 1,
  '4:5': 4 / 5,
};

export const SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];
/** export resolution choices: short side in px (null = keep) */
export const SHORT_SIDES: (number | null)[] = [null, 1080, 720, 480];

export interface EditState {
  /** source seconds */
  trim: { start: number; end: number };
  /** source pixels in displayed orientation (before the user rotation) */
  crop: Rect | null;
  aspect: AspectKey;
  rotate: Rotation;
  flipH: boolean;
  flipV: boolean;
  speed: number;
  volume: number;
  mute: boolean;
  adjust: VideoAdjust;
  /** geometry in "design" pixels = base size (crop + rotate, before output scaling); start/end on the output timeline */
  texts: TextLayer[];
  /** `short` = target short side (null = original); converted to the server's output height on export */
  output: { format: Format; short: number | null; quality: Quality };
}

export const NO_ADJUST: VideoAdjust = { brightness: 0, contrast: 0, saturation: 0, hue: 0, warmth: 0, vignette: 0 };

export function initialState(duration: number): EditState {
  return {
    trim: { start: 0, end: Math.max(0.1, duration) },
    crop: null,
    aspect: 'free',
    rotate: 0,
    flipH: false,
    flipV: false,
    speed: 1,
    volume: 1,
    mute: false,
    adjust: { ...NO_ADJUST },
    texts: [],
    output: { format: 'mp4', short: null, quality: 'high' },
  };
}

export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const even = (n: number) => Math.max(2, Math.round(n / 2) * 2);
const evenFloor = (n: number) => Math.max(2, Math.floor(n / 2) * 2);

// ───────────────────────── Geometry ─────────────────────────

/** Integer crop clamped to the frame (null when it covers the whole frame) — same rules as the server. */
export function normalizeCrop(crop: Rect | null, srcW: number, srcH: number): Rect | null {
  if (!crop) return null;
  const x = clamp(Math.round(crop.x), 0, srcW - 2);
  const y = clamp(Math.round(crop.y), 0, srcH - 2);
  const w = clamp(Math.round(crop.w), 2, srcW - x);
  const h = clamp(Math.round(crop.h), 2, srcH - y);
  if (x === 0 && y === 0 && w === srcW && h === srcH) return null;
  return { x, y, w, h };
}

export const rotated = (r: Rotation) => r === 90 || r === 270;

/** Size after crop + rotate (the text design space). */
export function baseSize(src: Size, crop: Rect | null, rotate: Rotation): Size {
  const c = normalizeCrop(crop, src.w, src.h);
  const w = c ? c.w : src.w;
  const h = c ? c.h : src.h;
  return rotated(rotate) ? { w: h, h: w } : { w, h };
}

/** Final output size — mirrors planVideoEdit on the server. */
export function outputSize(src: Size, crop: Rect | null, rotate: Rotation, height: number | null): Size {
  const b = baseSize(src, crop, rotate);
  if (height) {
    const h = even(height);
    return { w: even((b.w * h) / b.h), h };
  }
  return { w: evenFloor(b.w), h: evenFloor(b.h) };
}

/** Output height for a short-side choice (720p of a portrait clip = 720 px wide). */
export function targetHeight(base: Size, short: number | null): number | null {
  if (!short) return null;
  return base.h <= base.w ? short : Math.round((short * base.h) / base.w);
}

/** Output size of the current edit (what the exported file will measure). */
export function exportSize(src: Size, s: Pick<EditState, 'crop' | 'rotate' | 'output'>): Size {
  return outputSize(src, s.crop, s.rotate, targetHeight(baseSize(src, s.crop, s.rotate), s.output.short));
}

/** Source point → oriented (rotated + flipped) point. `src` is the unrotated frame size. */
export function orientPoint(x: number, y: number, src: Size, rot: Rotation, flipH: boolean, flipV: boolean): [number, number] {
  let u: number;
  let v: number;
  if (rot === 90) [u, v] = [src.h - y, x];
  else if (rot === 180) [u, v] = [src.w - x, src.h - y];
  else if (rot === 270) [u, v] = [y, src.w - x];
  else [u, v] = [x, y];
  const o = rotated(rot) ? { w: src.h, h: src.w } : src;
  if (flipH) u = o.w - u;
  if (flipV) v = o.h - v;
  return [u, v];
}

/** Oriented point → source point (inverse of orientPoint). */
export function unorientPoint(u: number, v: number, src: Size, rot: Rotation, flipH: boolean, flipV: boolean): [number, number] {
  const o = rotated(rot) ? { w: src.h, h: src.w } : src;
  if (flipH) u = o.w - u;
  if (flipV) v = o.h - v;
  if (rot === 90) return [v, src.h - u];
  if (rot === 180) return [src.w - u, src.h - v];
  if (rot === 270) return [src.w - v, u];
  return [u, v];
}

function rectFromPoints(a: [number, number], b: [number, number]): Rect {
  return { x: Math.min(a[0], b[0]), y: Math.min(a[1], b[1]), w: Math.abs(a[0] - b[0]), h: Math.abs(a[1] - b[1]) };
}

export function orientRect(r: Rect, src: Size, rot: Rotation, flipH: boolean, flipV: boolean): Rect {
  return rectFromPoints(orientPoint(r.x, r.y, src, rot, flipH, flipV), orientPoint(r.x + r.w, r.y + r.h, src, rot, flipH, flipV));
}

export function unorientRect(r: Rect, src: Size, rot: Rotation, flipH: boolean, flipV: boolean): Rect {
  return rectFromPoints(unorientPoint(r.x, r.y, src, rot, flipH, flipV), unorientPoint(r.x + r.w, r.y + r.h, src, rot, flipH, flipV));
}

/** Largest centred rect of `ratio` (w/h) inside `bounds`. */
export function fitAspect(bounds: Size, ratio: number): Rect {
  let w = bounds.w;
  let h = w / ratio;
  if (h > bounds.h) {
    h = bounds.h;
    w = h * ratio;
  }
  return { x: (bounds.w - w) / 2, y: (bounds.h - h) / 2, w, h };
}

/** Fit a box of `aspect` into `avail` (contain). */
export function containSize(avail: Size, aspect: number): Size {
  if (avail.w <= 0 || avail.h <= 0) return { w: 0, h: 0 };
  return avail.w / avail.h > aspect ? { w: avail.h * aspect, h: avail.h } : { w: avail.w, h: avail.w / aspect };
}

/** Keep text layers in place (relative to the frame) when the design space changes size. */
export function remapTexts(texts: TextLayer[], from: Size, to: Size): TextLayer[] {
  if (from.w === to.w && from.h === to.h) return texts;
  const rx = to.w / from.w;
  const ry = to.h / from.h;
  // geometric mean: reversible (crop on → off restores sizes) and neutral for 90° rotations
  const k = Math.sqrt(rx * ry);
  return texts.map((l) => ({
    ...l,
    x: l.x * rx,
    y: l.y * ry,
    fontSize: Math.max(4, l.fontSize * k),
    letterSpacing: l.letterSpacing * k,
    stroke: l.stroke ? { ...l.stroke, width: l.stroke.width * k } : null,
    shadow: l.shadow ? { ...l.shadow, blur: l.shadow.blur * k, x: l.shadow.x * k, y: l.shadow.y * k } : null,
    background: l.background ? { ...l.background, padding: l.background.padding * k, radius: l.background.radius * k } : null,
  }));
}

// ───────────────────────── Time ─────────────────────────

export function outDuration(s: Pick<EditState, 'trim' | 'speed'>): number {
  return Math.max(0, (s.trim.end - s.trim.start) / s.speed);
}

export const srcToOut = (s: Pick<EditState, 'trim' | 'speed'>, t: number) => (t - s.trim.start) / s.speed;
export const outToSrc = (s: Pick<EditState, 'trim' | 'speed'>, t: number) => s.trim.start + t * s.speed;

/** Visible window of a text layer on the output timeline. */
export function textWindow(l: TextLayer, dur: number): { start: number; end: number } {
  const start = clamp(l.start ?? 0, 0, dur);
  const end = clamp(l.end ?? dur, start, dur);
  return { start, end };
}

// ───────────────────────── Colour preview ─────────────────────────

/** Measured channel gains of ffmpeg `colortemperature=K:pl=1` for K = 6500 − 2500·warmth. */
const WARMTH_TABLE: [number, number, number, number][] = [
  [-1, 0.898, 0.953, 1.094],
  [-0.5, 0.9375, 0.96, 1.055],
  [0, 1, 1, 1],
  [0.5, 1.086, 0.992, 0.906],
  [1, 1.21, 0.977, 0.78],
];

export function warmthGains(w: number): [number, number, number] {
  const x = clamp(w, -1, 1);
  for (let i = 0; i < WARMTH_TABLE.length - 1; i++) {
    const a = WARMTH_TABLE[i];
    const b = WARMTH_TABLE[i + 1];
    if (x <= b[0]) {
      const f = (x - a[0]) / (b[0] - a[0]);
      return [a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f, a[3] + (b[3] - a[3]) * f];
    }
  }
  return [1, 1, 1];
}

/**
 * SVG colour matrix approximating ffmpeg `eq` (brightness 0.3·b, contrast 1+c on luma,
 * saturation 1+s on chroma) in full-range BT.709 RGB.
 */
export function eqMatrix(a: VideoAdjust): number[] {
  const c = 1 + a.contrast;
  const s = 1 + a.saturation;
  const mid = 0.509; // eq pivots on code value 127.5 of limited-range luma
  const off = (1 - c) * mid + 1.164 * 0.3 * a.brightness;
  const W = [0.2126, 0.7152, 0.0722];
  const k = c - s;
  const row = (i: number) => [0, 1, 2].map((j) => (i === j ? s : 0) + k * W[j]);
  return [...row(0), 0, off, ...row(1), 0, off, ...row(2), 0, off, 0, 0, 0, 1, 0];
}

/** Opacity stops of a black radial overlay equal to ffmpeg `vignette=angle=v·π/2.5` (cos⁴ fall-off). */
export function vignetteGradient(v: number): string | null {
  if (v <= 0.001) return null;
  const angle = (v * Math.PI) / 2.5;
  const stops: string[] = [];
  for (let i = 0; i <= 10; i++) {
    const d = i / 10;
    const c = Math.cos(angle * d);
    const alpha = clamp(1 - c * c * c * c, 0, 1);
    stops.push(`rgba(0,0,0,${alpha.toFixed(3)}) ${i * 10}%`);
  }
  return `radial-gradient(circle farthest-corner at 50% 50%, ${stops.join(', ')})`;
}

export function isAdjusted(a: VideoAdjust): boolean {
  return Object.values(a).some((v) => Math.abs(v) > 1e-4);
}

// ───────────────────────── Export ─────────────────────────

/** Group text layers with the same visible window → one overlay PNG each. */
export function overlayGroups(texts: TextLayer[], dur: number): { start: number; end: number; layers: TextLayer[] }[] {
  const groups = new Map<string, { start: number; end: number; layers: TextLayer[] }>();
  for (const l of texts) {
    if (!l.text.trim() || l.opacity <= 0) continue;
    const w = textWindow(l, dur);
    if (w.end - w.start < 0.02) continue;
    const key = `${w.start.toFixed(3)}-${w.end.toFixed(3)}`;
    const g = groups.get(key) ?? { ...w, layers: [] };
    g.layers.push(l);
    groups.set(key, g);
  }
  return [...groups.values()];
}

export function buildSpec(sourceId: string, s: EditState, src: Size, duration: number, overlays: VideoOverlay[]): VideoEditSpec {
  const crop = normalizeCrop(s.crop, src.w, src.h);
  const trimmed = s.trim.start > 0.01 || s.trim.end < duration - 0.01;
  return {
    sourceId,
    ...(trimmed ? { trim: { start: round3(s.trim.start), end: round3(s.trim.end) } } : {}),
    ...(crop ? { crop } : {}),
    rotate: s.rotate,
    flipH: s.flipH,
    flipV: s.flipV,
    speed: s.speed,
    volume: s.volume,
    mute: s.mute,
    adjust: s.adjust,
    overlays,
    output: { format: s.output.format, quality: s.output.quality, height: targetHeight(baseSize(src, s.crop, s.rotate), s.output.short) },
  };
}

const round3 = (n: number) => Math.round(n * 1000) / 1000;

export function baseName(name: string): string {
  const i = name.lastIndexOf('.');
  return i > 0 ? name.slice(0, i) : name;
}

// ───────────────────────── Crop box interaction ─────────────────────────

export type Handle = 'move' | 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';

/**
 * Resize/move a crop rect (oriented source pixels) by a pointer delta, keeping it inside
 * `b`, at least `min` px, and — when `ratio` is set — at that aspect ratio.
 */
export function resizeRect(r: Rect, handle: Handle, dx: number, dy: number, b: Size, ratio: number | null, min: number): Rect {
  if (handle === 'move') return { ...r, x: clamp(r.x + dx, 0, b.w - r.w), y: clamp(r.y + dy, 0, b.h - r.h) };
  const east = handle.includes('e');
  const west = handle.includes('w');
  const south = handle.includes('s');
  const north = handle.includes('n');
  if (!ratio) {
    let x0 = r.x;
    let y0 = r.y;
    let x1 = r.x + r.w;
    let y1 = r.y + r.h;
    if (west) x0 = clamp(x0 + dx, 0, x1 - min);
    if (east) x1 = clamp(x1 + dx, x0 + min, b.w);
    if (north) y0 = clamp(y0 + dy, 0, y1 - min);
    if (south) y1 = clamp(y1 + dy, y0 + min, b.h);
    return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
  }
  const cx = r.x + r.w / 2;
  const cy = r.y + r.h / 2;
  const dirX = east ? 1 : west ? -1 : 0;
  const dirY = south ? 1 : north ? -1 : 0;
  const ax = dirX === 1 ? r.x : dirX === -1 ? r.x + r.w : cx;
  const ay = dirY === 1 ? r.y : dirY === -1 ? r.y + r.h : cy;
  let w = dirX ? Math.abs((dirX === 1 ? r.x + r.w + dx : r.x + dx) - ax) : 0;
  const h0 = dirY ? Math.abs((dirY === 1 ? r.y + r.h + dy : r.y + dy) - ay) : 0;
  if (dirX && dirY) w = Math.max(w, h0 * ratio);
  else if (!dirX) w = h0 * ratio;
  const maxW = dirX === 1 ? b.w - ax : dirX === -1 ? ax : 2 * Math.min(cx, b.w - cx);
  const maxH = dirY === 1 ? b.h - ay : dirY === -1 ? ay : 2 * Math.min(cy, b.h - cy);
  const hi = Math.min(maxW, maxH * ratio);
  w = Math.min(hi, Math.max(w, min, min * ratio));
  const h = w / ratio;
  const x = dirX === 1 ? ax : dirX === -1 ? ax - w : cx - w / 2;
  const y = dirY === 1 ? ay : dirY === -1 ? ay - h : cy - h / 2;
  return { x, y, w, h };
}
