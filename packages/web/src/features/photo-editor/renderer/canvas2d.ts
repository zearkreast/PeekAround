import { straightenZoom } from '../geometry';
import type { RenderAdjust } from '../adjust';
import type { RenderParams, Renderer } from './types';

/** Approximate the shader with CSS filters (used only when WebGL2 is unavailable). */
function cssFilter(a: RenderAdjust, blurPx: number) {
  const parts: string[] = [];
  const bright = Math.pow(2, (a.exposure * 1.6) / 2.2) * (1 + a.brightness * 0.35) * (1 + a.shadows * 0.08 + a.highlights * 0.08);
  if (Math.abs(bright - 1) > 0.001) parts.push(`brightness(${bright.toFixed(3)})`);
  const ck = (a.contrast >= 0 ? 1 + a.contrast * 1.1 : 1 + a.contrast * 0.75) * (1 - a.fade * 0.25);
  if (Math.abs(ck - 1) > 0.001) parts.push(`contrast(${ck.toFixed(3)})`);
  const sat = Math.max(0, 1 + a.saturation + a.vibrance * 0.5);
  if (Math.abs(sat - 1) > 0.001) parts.push(`saturate(${sat.toFixed(3)})`);
  if (a.temperature > 0) parts.push(`sepia(${(a.temperature * 0.35).toFixed(3)})`);
  const hue = (a.hue * 180) / Math.PI + (a.temperature < 0 ? a.temperature * 12 : 0) + a.tint * 8;
  if (Math.abs(hue) > 0.01) parts.push(`hue-rotate(${hue.toFixed(1)}deg)`);
  if (blurPx > 0.2) parts.push(`blur(${blurPx.toFixed(2)}px)`);
  return parts.join(' ') || 'none';
}

export function createCanvasRenderer(): Renderer {
  const canvas = document.createElement('canvas');
  const ctx = canvas.getContext('2d')!;
  let src: { img: CanvasImageSource; w: number; h: number } | null = null;
  return {
    kind: 'canvas2d',
    canvas,
    maxSize: 16384,
    setSource(img, w, h) {
      src = { img, w, h };
    },
    hasSource: () => !!src,
    render(p: RenderParams, width, height) {
      width = Math.max(1, Math.round(width));
      height = Math.max(1, Math.round(height));
      if (canvas.width !== width) canvas.width = width;
      if (canvas.height !== height) canvas.height = height;
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, width, height);
      if (!src) return;
      const a = p.adjust;
      // frame → canvas
      ctx.setTransform(1 / p.k, 0, 0, 1 / p.k, -p.ox / p.k, -p.oy / p.k);
      ctx.save();
      ctx.beginPath();
      ctx.rect(p.clip.x, p.clip.y, p.clip.w, p.clip.h);
      ctx.clip();
      const blurSrc = a.blur > 0 ? a.blur * Math.min(p.srcW, p.srcH) * 0.025 * 0.5 : 0;
      ctx.filter = cssFilter(a, blurSrc / p.k);
      // straighten around the frame centre
      ctx.translate(p.frameW / 2, p.frameH / 2);
      ctx.rotate((p.straighten * Math.PI) / 180);
      const z = straightenZoom(p.frameW, p.frameH, p.straighten);
      ctx.scale(z, z);
      ctx.translate(-p.frameW / 2, -p.frameH / 2);
      if (p.flipH) ctx.transform(-1, 0, 0, 1, p.frameW, 0);
      if (p.flipV) ctx.transform(1, 0, 0, -1, 0, p.frameH);
      if (p.rot === 1) ctx.transform(0, 1, -1, 0, p.srcH, 0);
      else if (p.rot === 2) ctx.transform(-1, 0, 0, -1, p.srcW, p.srcH);
      else if (p.rot === 3) ctx.transform(0, -1, 1, 0, 0, p.srcW);
      ctx.imageSmoothingQuality = 'high';
      ctx.drawImage(src.img, 0, 0, p.srcW, p.srcH);
      ctx.restore();
      ctx.filter = 'none';
      if (Math.abs(a.vignette) > 0.001) {
        const cx = p.crop.x + p.crop.w / 2;
        const cy = p.crop.y + p.crop.h / 2;
        const r = Math.hypot(p.crop.w, p.crop.h) / 2;
        const g = ctx.createRadialGradient(cx, cy, r * 0.3, cx, cy, r * 1.12);
        const col = a.vignette > 0 ? '0,0,0' : '255,255,255';
        g.addColorStop(0, `rgba(${col},0)`);
        g.addColorStop(1, `rgba(${col},${Math.abs(a.vignette) * 0.85})`);
        ctx.save();
        ctx.beginPath();
        ctx.rect(p.clip.x, p.clip.y, p.clip.w, p.clip.h);
        ctx.clip();
        ctx.fillStyle = g;
        ctx.fillRect(p.crop.x, p.crop.y, p.crop.w, p.crop.h);
        ctx.restore();
      }
    },
    dispose() {
      src = null;
      canvas.width = canvas.height = 1;
    },
  };
}
