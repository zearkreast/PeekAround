import type { MediaItem } from '@peekaround/shared';
import type { Backend } from '@/backend';
import { ensureFont } from '@/lib/fonts';
import { drawTextLayer } from '@/lib/text';
import { canvasToBlob } from '@/features/photo-editor/exporter';
import { canvasGradient, coverRect, layoutCells, placeImage } from './geometry';
import { DESIGN_LONG, designSize, GRADIENTS, type CollageDoc } from './model';

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((res, rej) => {
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.decoding = 'async';
    img.onload = () => res(img);
    img.onerror = () => rej(new Error(`image failed: ${url}`));
    img.src = url;
  });
}

function roundRectPath(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  const rr = Math.max(0, Math.min(r, w / 2, h / 2));
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.arcTo(x + w, y, x + w, y + h, rr);
  ctx.arcTo(x + w, y + h, x, y + h, rr);
  ctx.arcTo(x, y + h, x, y, rr);
  ctx.arcTo(x, y, x + w, y, rr);
  ctx.closePath();
}

const supportsFilter = () => typeof CanvasRenderingContext2D !== 'undefined' && 'filter' in CanvasRenderingContext2D.prototype;

/** Draw a blurred, slightly darkened cover image (the "blurred photo" background). */
function drawBlurred(ctx: CanvasRenderingContext2D, img: CanvasImageSource & { width: number; height: number }, W: number, H: number, blur: number) {
  const r = coverRect(W, H, img.width, img.height, 1.12);
  if (supportsFilter()) {
    ctx.save();
    ctx.filter = `blur(${blur}px)`;
    ctx.drawImage(img, r.x, r.y, r.w, r.h);
    ctx.restore();
  } else {
    // fallback: down-sample → up-sample
    const k = Math.max(0.01, Math.min(1, 6 / Math.max(1, blur)));
    const c = document.createElement('canvas');
    c.width = Math.max(1, Math.round(W * k));
    c.height = Math.max(1, Math.round(H * k));
    c.getContext('2d')!.drawImage(img, r.x * k, r.y * k, r.w * k, r.h * k);
    ctx.save();
    ctx.imageSmoothingQuality = 'high';
    ctx.drawImage(c, 0, 0, W, H);
    ctx.restore();
  }
  ctx.fillStyle = 'rgba(0,0,0,0.12)';
  ctx.fillRect(0, 0, W, H);
}

/**
 * Render the collage at its output resolution. Cells use the same geometry as the preview;
 * full-size images are loaded one at a time to keep memory in check.
 */
export async function renderCollage(doc: CollageDoc, photos: Map<string, MediaItem>, backend: Backend, onProgress?: (p: number) => void): Promise<Blob> {
  const { W, H } = designSize(doc.aspect);
  const scale = doc.longSide / DESIGN_LONG;
  const canvas = document.createElement('canvas');
  canvas.width = Math.round(W * scale);
  canvas.height = Math.round(H * scale);
  const ctx = canvas.getContext('2d');
  if (!ctx) throw new Error('canvas unavailable');
  ctx.imageSmoothingQuality = 'high';
  ctx.setTransform(scale, 0, 0, scale, 0, 0);

  const cells = layoutCells(doc);
  const slotsUsed = cells.map((c) => doc.slots[c.index]).filter((s) => s?.id);

  if (doc.bg.kind === 'solid') {
    ctx.fillStyle = doc.bg.color;
    ctx.fillRect(0, 0, W, H);
  } else if (doc.bg.kind === 'gradient') {
    const g = GRADIENTS.find((x) => x.id === (doc.bg as { id: string }).id) ?? GRADIENTS[0];
    ctx.fillStyle = canvasGradient(ctx, g, W, H);
    ctx.fillRect(0, 0, W, H);
  } else {
    ctx.fillStyle = '#111';
    ctx.fillRect(0, 0, W, H);
    const first = slotsUsed[0]?.id;
    if (first) {
      const img = await loadImage(backend.thumbUrl(first, 1600));
      drawBlurred(ctx, img, W, H, doc.bg.amount);
    }
  }
  onProgress?.(0.08);

  let done = 0;
  for (const c of cells) {
    const slot = doc.slots[c.index];
    const item = slot?.id ? photos.get(slot.id) : undefined;
    ctx.save();
    ctx.translate(c.x + c.w / 2, c.y + c.h / 2);
    ctx.rotate((c.rot * Math.PI) / 180);
    ctx.translate(-c.w / 2, -c.h / 2);
    if (c.free) {
      // paper frame with a soft drop shadow
      ctx.save();
      ctx.shadowColor = 'rgba(0,0,0,0.35)';
      ctx.shadowBlur = 24 * scale;
      ctx.shadowOffsetY = 8 * scale;
      ctx.fillStyle = '#fbfbf8';
      roundRectPath(ctx, 0, 0, c.w, c.h, c.radius);
      ctx.fill();
      ctx.restore();
    }
    roundRectPath(ctx, c.inner.x, c.inner.y, c.inner.w, c.inner.h, c.innerRadius);
    ctx.clip();
    if (item && slot) {
      const img = await loadImage(backend.displayUrl(item));
      const r = placeImage(c.inner, { w: img.naturalWidth, h: img.naturalHeight }, slot);
      ctx.drawImage(img, c.inner.x + r.x, c.inner.y + r.y, r.w, r.h);
    } else {
      ctx.fillStyle = 'rgba(127,127,127,0.18)';
      ctx.fillRect(c.inner.x, c.inner.y, c.inner.w, c.inner.h);
    }
    ctx.restore();
    done++;
    onProgress?.(0.08 + (done / cells.length) * 0.72);
  }

  // text on top (drawn in output pixels so shadows scale too)
  await Promise.all(doc.layers.map((l) => ensureFont(l.fontFamily, l.fontWeight, l.italic)));
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  for (const l of doc.layers) drawTextLayer(ctx, l, scale);
  onProgress?.(0.85);

  const blob = await canvasToBlob(canvas, 'image/png');
  canvas.width = canvas.height = 1;
  return blob;
}
