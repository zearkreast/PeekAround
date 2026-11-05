import { createContext, useContext } from 'react';
import type { MediaDetails } from '@peekaround/shared';
import { cropPx, frameSize } from './geometry';
import type { Renderer } from './renderer';
import { useEditor } from './store';

export type SourceImage = ImageBitmap | HTMLImageElement;

/** Things every editor component needs that aren't part of the (undoable) document. */
export interface EditorEnv {
  item: MediaDetails;
  srcW: number;
  srcH: number;
  /** full resolution, decoded (null while loading) */
  source: SourceImage | null;
  /** ≤ 320px copy for filter thumbnails */
  small: HTMLCanvasElement | null;
  renderer: Renderer | null;
}

export const EditorEnvContext = createContext<EditorEnv | null>(null);

export function useEnv(): EditorEnv {
  const e = useContext(EditorEnvContext);
  if (!e) throw new Error('EditorEnvContext missing');
  return e;
}

/** Frame and crop (frame px) of the current document. */
export function useFrame() {
  const env = useEnv();
  const geo = useEditor((s) => s.h.doc.geo);
  const frame = frameSize(env.srcW, env.srcH, geo.rot);
  return { frame, crop: cropPx(geo, frame.w, frame.h) };
}

/** Grain cell size (frame px) at full resolution — preview uses max(this, 1 screen px). */
export function grainCellFor(crop: { w: number; h: number }) {
  return Math.max(1, Math.min(crop.w, crop.h) / 1600);
}
