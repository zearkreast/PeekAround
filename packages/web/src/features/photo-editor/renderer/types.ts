import type { RenderAdjust } from '../adjust';
import type { Rect } from '../types';

export interface RenderParams {
  adjust: RenderAdjust;
  /** natural source size in pixels (texture may be smaller) */
  srcW: number;
  srcH: number;
  rot: 0 | 1 | 2 | 3;
  flipH: boolean;
  flipV: boolean;
  /** degrees */
  straighten: number;
  frameW: number;
  frameH: number;
  /** canvas pixel (0,0) maps to frame point (ox, oy); one canvas pixel = k frame pixels */
  ox: number;
  oy: number;
  k: number;
  /** pixels outside this frame rect are transparent */
  clip: Rect;
  /** output rect (vignette reference), frame pixels */
  crop: Rect;
  /** grain cell size in frame pixels */
  grainCell: number;
  /** sharpen kernel step in source pixels */
  sharpenStep: number;
}

export interface Renderer {
  readonly kind: 'webgl2' | 'canvas2d';
  readonly canvas: HTMLCanvasElement;
  /** largest canvas side this renderer can draw in one pass */
  readonly maxSize: number;
  setSource(img: HTMLImageElement | HTMLCanvasElement | ImageBitmap, w: number, h: number): void;
  hasSource(): boolean;
  render(p: RenderParams, width: number, height: number): void;
  dispose(): void;
}
