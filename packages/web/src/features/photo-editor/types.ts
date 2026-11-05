import type { TextLayer } from '@/lib/text';

/** Adjustment sliders, all in "slider units" (mostly −100…100, hue in degrees). 0 = untouched. */
export interface Adjust {
  exposure: number;
  brightness: number;
  contrast: number;
  highlights: number;
  shadows: number;
  saturation: number;
  vibrance: number;
  temperature: number;
  tint: number;
  hue: number;
  fade: number;
  vignette: number;
  grain: number;
  sharpen: number;
  blur: number;
}

export type AdjustKey = keyof Adjust;

export const NEUTRAL_ADJUST: Adjust = {
  exposure: 0,
  brightness: 0,
  contrast: 0,
  highlights: 0,
  shadows: 0,
  saturation: 0,
  vibrance: 0,
  temperature: 0,
  tint: 0,
  hue: 0,
  fade: 0,
  vignette: 0,
  grain: 0,
  sharpen: 0,
  blur: 0,
};

/** Split toning colour offsets (added in RGB, roughly −0.15…0.15). */
export interface SplitTone {
  shadows: [number, number, number];
  highlights: [number, number, number];
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type AspectKey = 'free' | 'original' | '1:1' | '4:5' | '3:4' | '16:9' | '9:16';

/**
 * Geometry. The "frame" is the source image after 90° rotations + flips (frameW × frameH,
 * in source pixels). Straighten rotates the image inside the frame and zooms so the frame
 * stays covered. The crop is a normalized rect inside the frame.
 */
export interface Geo {
  rot: 0 | 1 | 2 | 3; // clockwise quarter turns
  flipH: boolean;
  flipV: boolean;
  straighten: number; // degrees, −45…45
  crop: Rect; // normalized 0…1 within the frame
  aspect: AspectKey;
}

/** Freehand brush stroke; points are in frame pixels. */
export interface Stroke {
  id: string;
  color: string;
  size: number; // frame pixels
  opacity: number;
  points: number[];
}

/** Everything that defines an edit — the unit of undo/redo. */
export interface EditDoc {
  adjust: Adjust;
  filter: { id: string | null; strength: number };
  geo: Geo;
  /** text layers — x/y/fontSize in frame pixels */
  layers: TextLayer[];
  strokes: Stroke[];
}

export const FULL_CROP: Rect = { x: 0, y: 0, w: 1, h: 1 };

export function emptyDoc(): EditDoc {
  return {
    adjust: { ...NEUTRAL_ADJUST },
    filter: { id: null, strength: 100 },
    geo: { rot: 0, flipH: false, flipV: false, straighten: 0, crop: { ...FULL_CROP }, aspect: 'free' },
    layers: [],
    strokes: [],
  };
}

export type EditorTab = 'adjust' | 'filters' | 'crop' | 'text' | 'draw';
