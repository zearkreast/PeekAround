import { ensureFont } from '@/lib/fonts';
import { drawTextLayer } from '@/lib/text';
import { effectiveAdjust } from './adjust';
import { grainCellFor, type SourceImage } from './context';
import { cropPx, frameSize } from './geometry';
import { createRenderer } from './renderer';
import { drawStroke } from './strokes';
import type { EditDoc } from './types';

const nextFrame = () => new Promise<void>((r) => requestAnimationFrame(() => r()));

export function canvasToBlob(c: HTMLCanvasElement, type = 'image/png', quality?: number): Promise<Blob> {
  return new Promise((res, rej) => c.toBlob((b) => (b ? res(b) : rej(new Error('toBlob failed'))), type, quality));
}

/**
 * Render the edit at FULL resolution: adjustments + geometry through the same shader as the
 * preview (in tiles, so huge photos fit GPU limits), then strokes and text on top. Returns a
 * lossless PNG — the server encodes it to the format the user picked.
 */
export async function renderEdit(source: SourceImage, srcW: number, srcH: number, doc: EditDoc, onProgress?: (p: number) => void): Promise<Blob> {
  const frame = frameSize(srcW, srcH, doc.geo.rot);
  const crop = cropPx(doc.geo, frame.w, frame.h);
  await Promise.all(doc.layers.map((l) => ensureFont(l.fontFamily, l.fontWeight, l.italic)));

  const out = document.createElement('canvas');
  out.width = crop.w;
  out.height = crop.h;
  const ctx = out.getContext('2d');
  if (!ctx) throw new Error('canvas unavailable');

  const r = createRenderer({ preserve: true });
  try {
    r.setSource(source, srcW, srcH);
    const tile = Math.min(r.maxSize, r.kind === 'webgl2' ? 2048 : 16384);
    const adjust = effectiveAdjust(doc.adjust, doc.filter);
    const cols = Math.ceil(crop.w / tile);
    const rows = Math.ceil(crop.h / tile);
    let done = 0;
    for (let ty = 0; ty < crop.h; ty += tile) {
      for (let tx = 0; tx < crop.w; tx += tile) {
        const tw = Math.min(tile, crop.w - tx);
        const th = Math.min(tile, crop.h - ty);
        r.render(
          {
            adjust,
            srcW,
            srcH,
            rot: doc.geo.rot,
            flipH: doc.geo.flipH,
            flipV: doc.geo.flipV,
            straighten: doc.geo.straighten,
            frameW: frame.w,
            frameH: frame.h,
            ox: crop.x + tx,
            oy: crop.y + ty,
            k: 1,
            clip: crop,
            crop,
            grainCell: grainCellFor(crop),
            sharpenStep: 1,
          },
          tw,
          th,
        );
        ctx.drawImage(r.canvas, 0, 0, tw, th, tx, ty, tw, th);
        done++;
        onProgress?.((done / (cols * rows)) * 0.8);
        await nextFrame();
      }
    }
  } finally {
    r.dispose();
  }

  ctx.save();
  ctx.translate(-crop.x, -crop.y);
  for (const s of doc.strokes) drawStroke(ctx, s);
  for (const l of doc.layers) drawTextLayer(ctx, l, 1);
  ctx.restore();
  onProgress?.(0.85);
  await nextFrame();
  const blob = await canvasToBlob(out, 'image/png');
  out.width = out.height = 1;
  return blob;
}
