import { createCanvasRenderer } from './canvas2d';
import { createGlRenderer } from './gl';
import type { Renderer } from './types';

export type { Renderer, RenderParams } from './types';

/** WebGL2 when available, canvas 2D (CSS filters) otherwise. */
export function createRenderer(opts: { preserve?: boolean } = {}): Renderer {
  return createGlRenderer(opts) ?? createCanvasRenderer();
}
