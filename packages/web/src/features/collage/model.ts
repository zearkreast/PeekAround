import type { TextLayer } from '@/lib/text';

/** Design space: the long side of the collage is always DESIGN_LONG units; export scales it. */
export const DESIGN_LONG = 2000;

export type CollageAspect = '1:1' | '4:5' | '9:16' | '16:9' | '3:2' | 'a4';

export const ASPECTS: { key: CollageAspect; ratio: number; label?: 'collage.story' }[] = [
  { key: '1:1', ratio: 1 },
  { key: '4:5', ratio: 4 / 5 },
  { key: '9:16', ratio: 9 / 16, label: 'collage.story' },
  { key: '16:9', ratio: 16 / 9 },
  { key: '3:2', ratio: 3 / 2 },
  { key: 'a4', ratio: 1 / Math.SQRT2 },
];

export function aspectRatioOf(a: CollageAspect) {
  return ASPECTS.find((x) => x.key === a)?.ratio ?? 1;
}

export function designSize(a: CollageAspect) {
  const r = aspectRatioOf(a);
  return r >= 1 ? { W: DESIGN_LONG, H: Math.round(DESIGN_LONG / r) } : { W: Math.round(DESIGN_LONG * r), H: DESIGN_LONG };
}

export const RESOLUTIONS = [2400, 3000, 4000] as const;

export interface GradientPreset {
  id: string;
  angle: number; // css degrees
  stops: [number, string][];
}

export const GRADIENTS: GradientPreset[] = [
  { id: 'aurora', angle: 135, stops: [[0, '#8b5cf6'], [0.55, '#ec4899'], [1, '#f97316']] },
  { id: 'ocean', angle: 160, stops: [[0, '#0ea5e9'], [1, '#1e3a8a']] },
  { id: 'peach', angle: 120, stops: [[0, '#fde68a'], [1, '#fb7185']] },
  { id: 'mint', angle: 140, stops: [[0, '#d9f99d'], [1, '#14b8a6']] },
  { id: 'night', angle: 180, stops: [[0, '#1e1b4b'], [1, '#020617']] },
  { id: 'sand', angle: 150, stops: [[0, '#fef3c7'], [1, '#d6b48a']] },
  { id: 'berry', angle: 135, stops: [[0, '#4c1d95'], [1, '#be185d']] },
  { id: 'mist', angle: 180, stops: [[0, '#f8fafc'], [1, '#cbd5e1']] },
];

export const SOLID_COLORS = ['#ffffff', '#f5f0e8', '#111114', '#1f2937', '#fde2e4', '#dbeafe', '#dcfce7', '#fef9c3'];

export type Background = { kind: 'solid'; color: string } | { kind: 'gradient'; id: string } | { kind: 'blur'; amount: number };

/** One collage cell's photo + framing inside the cell. */
export interface Slot {
  id: string | null;
  zoom: number; // ≥ 1
  px: number; // −1…1 pan (1 = show the left/top edge)
  py: number;
}

export interface CollageDoc {
  count: number;
  template: string;
  seed: number;
  aspect: CollageAspect;
  longSide: number;
  gap: number;
  padding: number;
  radius: number;
  bg: Background;
  slots: Slot[];
  /** design units */
  layers: TextLayer[];
}

export const emptySlot = (id: string | null = null): Slot => ({ id, zoom: 1, px: 0, py: 0 });

export function newDoc(ids: string[]): CollageDoc {
  const count = Math.min(9, Math.max(2, ids.length || 4));
  return {
    count,
    template: 'grid',
    seed: 7,
    aspect: '4:5',
    longSide: 3000,
    gap: 24,
    padding: 24,
    radius: 12,
    bg: { kind: 'solid', color: '#ffffff' },
    slots: Array.from({ length: 9 }, (_, i) => emptySlot(ids[i] ?? null)),
    layers: [],
  };
}

export function cssGradient(g: GradientPreset) {
  return `linear-gradient(${g.angle}deg, ${g.stops.map(([o, c]) => `${c} ${Math.round(o * 100)}%`).join(', ')})`;
}
