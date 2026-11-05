import type { TextLayer } from '@/lib/text';
import type { AspectKey, EditDoc, Geo, Rect, Stroke } from './types';

/** Frame size (after quarter turns) in source pixels. */
export function frameSize(srcW: number, srcH: number, rot: number) {
  return rot % 2 === 1 ? { w: srcH, h: srcW } : { w: srcW, h: srcH };
}

/** Zoom needed so a straightened (rotated) image still covers its whole frame. */
export function straightenZoom(frameW: number, frameH: number, deg: number) {
  const a = Math.abs((deg * Math.PI) / 180);
  return Math.cos(a) + Math.max(frameW / frameH, frameH / frameW) * Math.sin(a);
}

/** Crop rect in frame pixels (rounded to whole output pixels). */
export function cropPx(geo: Geo, frameW: number, frameH: number): Rect {
  const x = Math.round(geo.crop.x * frameW);
  const y = Math.round(geo.crop.y * frameH);
  const w = Math.max(1, Math.min(frameW - x, Math.round(geo.crop.w * frameW)));
  const h = Math.max(1, Math.min(frameH - y, Math.round(geo.crop.h * frameH)));
  return { x, y, w, h };
}

export const ASPECTS: { key: AspectKey; ratio: number | null }[] = [
  { key: 'free', ratio: null },
  { key: 'original', ratio: null },
  { key: '1:1', ratio: 1 },
  { key: '4:5', ratio: 4 / 5 },
  { key: '3:4', ratio: 3 / 4 },
  { key: '16:9', ratio: 16 / 9 },
  { key: '9:16', ratio: 9 / 16 },
];

/** Pixel aspect (w/h) for a preset, or null for free. */
export function aspectRatio(key: AspectKey, frameW: number, frameH: number): number | null {
  if (key === 'free') return null;
  if (key === 'original') return frameW / frameH;
  return ASPECTS.find((a) => a.key === key)?.ratio ?? null;
}

/** Largest crop with the given pixel aspect centred on the current crop's centre, inside the frame. */
export function fitAspect(crop: Rect, ratio: number, frameW: number, frameH: number): Rect {
  const cx = (crop.x + crop.w / 2) * frameW;
  const cy = (crop.y + crop.h / 2) * frameH;
  // biggest rect of this ratio that fits the frame…
  let w = frameW;
  let h = w / ratio;
  if (h > frameH) {
    h = frameH;
    w = h * ratio;
  }
  // …shrunk a little when the current crop is small, so presets feel "local"
  const area = crop.w * frameW * crop.h * frameH;
  if (area < w * h * 0.9) {
    const k = Math.sqrt(Math.max(area, w * h * 0.25) / (w * h));
    w *= k;
    h *= k;
  }
  const x = Math.min(Math.max(0, cx - w / 2), frameW - w);
  const y = Math.min(Math.max(0, cy - h / 2), frameH - h);
  return { x: x / frameW, y: y / frameH, w: w / frameW, h: h / frameH };
}

// ───────────── quarter turns & flips (remap crop, layers and strokes) ─────────────

type PointMap = (x: number, y: number) => [number, number];

function mapLayers(layers: TextLayer[], f: PointMap, rot: (r: number) => number): TextLayer[] {
  return layers.map((l) => {
    const [x, y] = f(l.x, l.y);
    return { ...l, x, y, rotation: rot(l.rotation) };
  });
}

function mapStrokes(strokes: Stroke[], f: PointMap): Stroke[] {
  return strokes.map((s) => {
    const pts: number[] = [];
    for (let i = 0; i < s.points.length; i += 2) {
      const [x, y] = f(s.points[i], s.points[i + 1]);
      pts.push(x, y);
    }
    return { ...s, points: pts };
  });
}

/** Rotate the whole edit 90° clockwise (dir = 1) or counter-clockwise (dir = −1). */
export function rotateDoc(doc: EditDoc, dir: 1 | -1, frameW: number, frameH: number): EditDoc {
  const g = doc.geo;
  const oneFlip = g.flipH !== g.flipV;
  const rot = ((((g.rot + (oneFlip ? -dir : dir)) % 4) + 4) % 4) as Geo['rot'];
  const c = g.crop;
  const crop: Rect = dir === 1 ? { x: 1 - c.y - c.h, y: c.x, w: c.h, h: c.w } : { x: c.y, y: 1 - c.x - c.w, w: c.h, h: c.w };
  // frame pixels: clockwise (x, y) → (H − y, x); counter-clockwise (x, y) → (y, W − x)
  const f: PointMap = dir === 1 ? (x, y) => [frameH - y, x] : (x, y) => [y, frameW - x];
  const aspect: AspectKey = g.aspect === '1:1' || g.aspect === 'free' ? g.aspect : 'free';
  return {
    ...doc,
    geo: { ...g, rot, crop, aspect },
    layers: mapLayers(doc.layers, f, (r) => r),
    strokes: mapStrokes(doc.strokes, f),
  };
}

export function flipDoc(doc: EditDoc, axis: 'h' | 'v', frameW: number, frameH: number): EditDoc {
  const g = doc.geo;
  const c = g.crop;
  const crop: Rect = axis === 'h' ? { ...c, x: 1 - c.x - c.w } : { ...c, y: 1 - c.y - c.h };
  const f: PointMap = axis === 'h' ? (x, y) => [frameW - x, y] : (x, y) => [x, frameH - y];
  return {
    ...doc,
    geo: { ...g, flipH: axis === 'h' ? !g.flipH : g.flipH, flipV: axis === 'v' ? !g.flipV : g.flipV, straighten: -g.straighten, crop },
    layers: mapLayers(doc.layers, f, (r) => -r),
    strokes: mapStrokes(doc.strokes, f),
  };
}

// ───────────── view (frame ↔ screen) ─────────────

/** What part of the frame is on screen: `scale` CSS px per frame px, centred on (cx, cy). */
export interface View {
  scale: number;
  cx: number;
  cy: number;
}

export function fitView(region: Rect, stageW: number, stageH: number, pad = 32): View {
  const s = Math.min((stageW - pad * 2) / region.w, (stageH - pad * 2) / region.h);
  return { scale: Math.max(0.01, s), cx: region.x + region.w / 2, cy: region.y + region.h / 2 };
}

export function lerpView(a: View, b: View, t: number): View {
  return {
    scale: Math.exp(Math.log(a.scale) + (Math.log(b.scale) - Math.log(a.scale)) * t),
    cx: a.cx + (b.cx - a.cx) * t,
    cy: a.cy + (b.cy - a.cy) * t,
  };
}

/** Keep the region at least partially on screen when zoomed/panned. */
export function clampView(v: View, region: Rect, stageW: number, stageH: number): View {
  const halfW = stageW / 2 / v.scale;
  const halfH = stageH / 2 / v.scale;
  const clampAxis = (c: number, lo: number, len: number, half: number) => {
    if (len <= half * 2) return lo + len / 2; // region smaller than the stage → centre it
    return Math.min(Math.max(c, lo + half), lo + len - half);
  };
  return { scale: v.scale, cx: clampAxis(v.cx, region.x, region.w, halfW), cy: clampAxis(v.cy, region.y, region.h, halfH) };
}

export function frameToScreen(v: View, stageW: number, stageH: number, x: number, y: number): [number, number] {
  return [(x - v.cx) * v.scale + stageW / 2, (y - v.cy) * v.scale + stageH / 2];
}

export function screenToFrame(v: View, stageW: number, stageH: number, x: number, y: number): [number, number] {
  return [(x - stageW / 2) / v.scale + v.cx, (y - stageH / 2) / v.scale + v.cy];
}
