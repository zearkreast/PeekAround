import { aspectRatioOf, designSize, type CollageDoc, type GradientPreset, type Slot } from './model';
import { templateById, type CellSpec } from './templates';

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A laid-out cell in design units. `inner` is the photo window relative to the cell. */
export interface CellGeom extends Box {
  index: number;
  rot: number;
  style: NonNullable<CellSpec['style']>;
  radius: number;
  inner: Box;
  innerRadius: number;
  free: boolean;
}

const EPS = 1e-6;

/** Resolve the template for the doc into concrete cell rectangles (design units). */
export function layoutCells(doc: Pick<CollageDoc, 'template' | 'count' | 'aspect' | 'seed' | 'gap' | 'padding' | 'radius'>): CellGeom[] {
  const { W, H } = designSize(doc.aspect);
  const tpl = templateById(doc.template);
  const p = doc.padding;
  const content = { x: p, y: p, w: W - p * 2, h: H - p * 2 };
  const specs = tpl.make(doc.count, content.w / content.h || aspectRatioOf(doc.aspect), doc.seed);
  const g = doc.gap / 2;
  return specs.map((c, index) => {
    const style = c.style ?? 'plain';
    if (tpl.kind === 'free') {
      const w = c.w * content.w;
      const h = c.h * content.h;
      const x = content.x + c.x * content.w - w / 2;
      const y = content.y + c.y * content.h - h / 2;
      const side = style === 'polaroid' ? w * 0.055 : Math.max(6, w * 0.03);
      const bottom = style === 'polaroid' ? w * 0.2 : side;
      return {
        index,
        x,
        y,
        w,
        h,
        rot: c.rot ?? 0,
        style,
        radius: Math.min(doc.radius * 0.35, w * 0.05),
        inner: { x: side, y: side, w: w - side * 2, h: h - side - bottom },
        innerRadius: Math.min(doc.radius * 0.2, w * 0.03),
        free: true,
      };
    }
    let x = content.x + c.x * content.w;
    let y = content.y + c.y * content.h;
    let w = c.w * content.w;
    let h = c.h * content.h;
    if (c.x > EPS) (x += g), (w -= g);
    if (c.x + c.w < 1 - EPS) w -= g;
    if (c.y > EPS) (y += g), (h -= g);
    if (c.y + c.h < 1 - EPS) h -= g;
    w = Math.max(1, w);
    h = Math.max(1, h);
    const radius = Math.min(doc.radius, w / 2, h / 2);
    return { index, x, y, w, h, rot: 0, style, radius, inner: { x: 0, y: 0, w, h }, innerRadius: radius, free: false };
  });
}

/** Where the photo is drawn inside its window (relative to the window), cover-fit + zoom + pan. */
export function placeImage(win: { w: number; h: number }, img: { w: number; h: number }, slot: Slot): Box {
  const s = Math.max(win.w / img.w, win.h / img.h) * Math.max(1, slot.zoom);
  const w = img.w * s;
  const h = img.h * s;
  return { x: (win.w - w) / 2 + (slot.px * (w - win.w)) / 2, y: (win.h - h) / 2 + (slot.py * (h - win.h)) / 2, w, h };
}

/** Canvas gradient equivalent of a CSS `linear-gradient(angle, …)` over a W×H box. */
export function canvasGradient(ctx: CanvasRenderingContext2D, g: GradientPreset, W: number, H: number) {
  const a = (g.angle * Math.PI) / 180;
  const dx = Math.sin(a);
  const dy = -Math.cos(a);
  const len = Math.abs(W * dx) + Math.abs(H * dy);
  const cx = W / 2;
  const cy = H / 2;
  const grad = ctx.createLinearGradient(cx - (dx * len) / 2, cy - (dy * len) / 2, cx + (dx * len) / 2, cy + (dy * len) / 2);
  for (const [o, c] of g.stops) grad.addColorStop(o, c);
  return grad;
}

/** Cover-fit a W×H box with an image (scaled by `over` to hide blurred edges). */
export function coverRect(W: number, H: number, iw: number, ih: number, over = 1) {
  const s = Math.max(W / iw, H / ih) * over;
  return { x: (W - iw * s) / 2, y: (H - ih * s) / 2, w: iw * s, h: ih * s };
}
