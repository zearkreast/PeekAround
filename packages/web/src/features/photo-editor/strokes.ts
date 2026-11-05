import type { Stroke } from './types';

/** Smooth freehand stroke through the points (midpoint quadratic curves). Same code for preview and export. */
export function drawStroke(ctx: CanvasRenderingContext2D, s: Stroke) {
  const p = s.points;
  if (p.length < 2) return;
  ctx.save();
  ctx.globalAlpha *= s.opacity;
  ctx.strokeStyle = s.color;
  ctx.fillStyle = s.color;
  ctx.lineWidth = s.size;
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  if (p.length <= 4) {
    // a dot (or tiny dash)
    ctx.beginPath();
    ctx.arc(p[0], p[1], s.size / 2, 0, Math.PI * 2);
    ctx.fill();
    if (p.length === 4) {
      ctx.beginPath();
      ctx.moveTo(p[0], p[1]);
      ctx.lineTo(p[2], p[3]);
      ctx.stroke();
    }
    ctx.restore();
    return;
  }
  ctx.beginPath();
  ctx.moveTo(p[0], p[1]);
  for (let i = 2; i < p.length - 2; i += 2) {
    const mx = (p[i] + p[i + 2]) / 2;
    const my = (p[i + 1] + p[i + 3]) / 2;
    ctx.quadraticCurveTo(p[i], p[i + 1], mx, my);
  }
  ctx.lineTo(p[p.length - 2], p[p.length - 1]);
  ctx.stroke();
  ctx.restore();
}

export function strokeBounds(s: Stroke) {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (let i = 0; i < s.points.length; i += 2) {
    x0 = Math.min(x0, s.points[i]);
    x1 = Math.max(x1, s.points[i]);
    y0 = Math.min(y0, s.points[i + 1]);
    y1 = Math.max(y1, s.points[i + 1]);
  }
  const r = s.size / 2;
  return { x: x0 - r, y: y0 - r, width: x1 - x0 + s.size, height: y1 - y0 + s.size };
}
