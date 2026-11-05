import { cssFont, ensureFont } from './fonts';

/**
 * A text layer shared by the photo editor, collage and video editor.
 * All geometry is in OUTPUT pixels (the final image/video resolution).
 * (x, y) is the centre of the text block.
 */
export interface TextLayer {
  id: string;
  text: string;
  fontFamily: string;
  fontWeight: number;
  italic: boolean;
  fontSize: number;
  color: string;
  align: 'left' | 'center' | 'right';
  lineHeight: number;
  letterSpacing: number;
  x: number;
  y: number;
  rotation: number;
  opacity: number;
  stroke: { color: string; width: number } | null;
  shadow: { color: string; blur: number; x: number; y: number } | null;
  background: { color: string; padding: number; radius: number } | null;
  /** video only: visible from/to (seconds on the output timeline) */
  start?: number;
  end?: number;
}

let idSeq = 0;

export function createTextLayer(partial: Partial<TextLayer> = {}): TextLayer {
  return {
    id: `t${Date.now().toString(36)}${(idSeq++).toString(36)}`,
    text: 'Merhaba!',
    fontFamily: 'Bricolage Grotesque Variable',
    fontWeight: 700,
    italic: false,
    fontSize: 96,
    color: '#ffffff',
    align: 'center',
    lineHeight: 1.15,
    letterSpacing: 0,
    x: 0,
    y: 0,
    rotation: 0,
    opacity: 1,
    stroke: null,
    shadow: { color: 'rgba(0,0,0,0.45)', blur: 18, x: 0, y: 4 },
    background: null,
    ...partial,
  };
}

export interface TextMetrics {
  width: number;
  height: number;
  lines: { text: string; width: number }[];
  lineHeightPx: number;
}

export function measureTextLayer(ctx: CanvasRenderingContext2D, l: TextLayer, scale = 1): TextMetrics {
  ctx.save();
  ctx.font = cssFont(l.fontFamily, l.fontWeight, l.italic, l.fontSize * scale);
  setLetterSpacing(ctx, l.letterSpacing * scale);
  const lines = (l.text || ' ').split('\n').map((t) => ({ text: t, width: ctx.measureText(t || ' ').width }));
  ctx.restore();
  const lineHeightPx = l.fontSize * scale * l.lineHeight;
  const width = Math.max(1, ...lines.map((x) => x.width));
  return { width, height: lineHeightPx * lines.length, lines, lineHeightPx };
}

function setLetterSpacing(ctx: CanvasRenderingContext2D, px: number) {
  const c = ctx as CanvasRenderingContext2D & { letterSpacing?: string };
  if ('letterSpacing' in c) c.letterSpacing = `${px}px`;
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  const rr = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.arcTo(x + w, y, x + w, y + h, rr);
  ctx.arcTo(x + w, y + h, x, y + h, rr);
  ctx.arcTo(x, y + h, x, y, rr);
  ctx.arcTo(x, y, x + w, y, rr);
  ctx.closePath();
}

/** Draw a text layer onto a 2D context. `scale` maps output pixels → canvas pixels. */
export function drawTextLayer(ctx: CanvasRenderingContext2D, l: TextLayer, scale = 1) {
  const m = measureTextLayer(ctx, l, scale);
  ctx.save();
  ctx.globalAlpha = l.opacity;
  ctx.translate(l.x * scale, l.y * scale);
  ctx.rotate((l.rotation * Math.PI) / 180);
  ctx.font = cssFont(l.fontFamily, l.fontWeight, l.italic, l.fontSize * scale);
  setLetterSpacing(ctx, l.letterSpacing * scale);
  ctx.textBaseline = 'middle';
  ctx.textAlign = l.align;

  const left = -m.width / 2;
  const top = -m.height / 2;
  if (l.background) {
    const p = l.background.padding * scale;
    ctx.save();
    ctx.fillStyle = l.background.color;
    roundRect(ctx, left - p, top - p, m.width + p * 2, m.height + p * 2, l.background.radius * scale);
    ctx.fill();
    ctx.restore();
  }
  const ax = l.align === 'left' ? left : l.align === 'right' ? -left : 0;
  m.lines.forEach((line, i) => {
    const y = top + m.lineHeightPx * (i + 0.5);
    if (l.shadow) {
      ctx.shadowColor = l.shadow.color;
      ctx.shadowBlur = l.shadow.blur * scale;
      ctx.shadowOffsetX = l.shadow.x * scale;
      ctx.shadowOffsetY = l.shadow.y * scale;
    }
    if (l.stroke && l.stroke.width > 0) {
      ctx.lineJoin = 'round';
      ctx.miterLimit = 2;
      ctx.strokeStyle = l.stroke.color;
      ctx.lineWidth = l.stroke.width * 2 * scale;
      ctx.strokeText(line.text, ax, y);
      ctx.shadowColor = 'transparent';
    }
    ctx.fillStyle = l.color;
    ctx.fillText(line.text, ax, y);
  });
  ctx.restore();
}

/** Bounding box (axis aligned, unrotated) of a layer in output pixels — for hit testing / handles. */
export function textBox(ctx: CanvasRenderingContext2D, l: TextLayer) {
  const m = measureTextLayer(ctx, l, 1);
  const p = l.background ? l.background.padding : 0;
  return { x: l.x - m.width / 2 - p, y: l.y - m.height / 2 - p, width: m.width + p * 2, height: m.height + p * 2 };
}

/** Render text layers to a transparent PNG of the given size (video overlays). */
export async function renderTextPng(layers: TextLayer[], width: number, height: number): Promise<Blob> {
  await Promise.all(layers.map((l) => ensureFont(l.fontFamily, l.fontWeight, l.italic)));
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext('2d')!;
  for (const l of layers) drawTextLayer(ctx, l, 1);
  return new Promise((res, rej) => canvas.toBlob((b) => (b ? res(b) : rej(new Error('toBlob failed'))), 'image/png'));
}
