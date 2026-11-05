import type { MessageKey } from '@/i18n';
import { NEUTRAL_ADJUST, type Adjust, type AdjustKey, type SplitTone } from './types';

export interface SliderDef {
  key: AdjustKey;
  label: MessageKey;
  min: number;
  max: number;
  /** css gradient for the little swatch next to colour sliders */
  hint?: string;
}

export const ADJUST_GROUPS: { title: MessageKey; sliders: SliderDef[] }[] = [
  {
    title: 'photo.group.light',
    sliders: [
      { key: 'exposure', label: 'photo.adj.exposure', min: -100, max: 100 },
      { key: 'brightness', label: 'photo.adj.brightness', min: -100, max: 100 },
      { key: 'contrast', label: 'photo.adj.contrast', min: -100, max: 100 },
      { key: 'highlights', label: 'photo.adj.highlights', min: -100, max: 100 },
      { key: 'shadows', label: 'photo.adj.shadows', min: -100, max: 100 },
    ],
  },
  {
    title: 'photo.group.color',
    sliders: [
      { key: 'saturation', label: 'photo.adj.saturation', min: -100, max: 100, hint: 'linear-gradient(90deg,#888,#f43f5e)' },
      { key: 'vibrance', label: 'photo.adj.vibrance', min: -100, max: 100, hint: 'linear-gradient(90deg,#9ca3af,#22d3ee,#a855f7)' },
      { key: 'temperature', label: 'photo.adj.temperature', min: -100, max: 100, hint: 'linear-gradient(90deg,#3b82f6,#f59e0b)' },
      { key: 'tint', label: 'photo.adj.tint', min: -100, max: 100, hint: 'linear-gradient(90deg,#22c55e,#d946ef)' },
      { key: 'hue', label: 'photo.adj.hue', min: -180, max: 180, hint: 'linear-gradient(90deg,#f00,#ff0,#0f0,#0ff,#00f,#f0f,#f00)' },
    ],
  },
  {
    title: 'photo.group.effects',
    sliders: [
      { key: 'fade', label: 'photo.adj.fade', min: 0, max: 100 },
      { key: 'vignette', label: 'photo.adj.vignette', min: -100, max: 100 },
      { key: 'grain', label: 'photo.adj.grain', min: 0, max: 100 },
      { key: 'sharpen', label: 'photo.adj.sharpen', min: 0, max: 100 },
      { key: 'blur', label: 'photo.adj.blur', min: 0, max: 100 },
    ],
  },
];

export const ADJUST_RANGE: Record<AdjustKey, [number, number]> = Object.fromEntries(
  ADJUST_GROUPS.flatMap((g) => g.sliders.map((s) => [s.key, [s.min, s.max]])),
) as Record<AdjustKey, [number, number]>;

export interface FilterPreset {
  id: string;
  label: MessageKey;
  adjust: Partial<Adjust>;
  split?: SplitTone;
}

export const FILTERS: FilterPreset[] = [
  { id: 'vivid', label: 'photo.filter.vivid', adjust: { contrast: 14, saturation: 22, vibrance: 34, shadows: 8, highlights: -6 } },
  { id: 'warm', label: 'photo.filter.warm', adjust: { temperature: 32, tint: 6, vibrance: 12, highlights: -8, exposure: 3 } },
  { id: 'cool', label: 'photo.filter.cool', adjust: { temperature: -34, tint: -4, saturation: -6, contrast: 6 } },
  { id: 'mono', label: 'photo.filter.mono', adjust: { saturation: -100, contrast: 12, shadows: 6 } },
  { id: 'noir', label: 'photo.filter.noir', adjust: { saturation: -100, contrast: 34, exposure: -6, shadows: -14, highlights: -14, vignette: 36, grain: 12 } },
  {
    id: 'vintage',
    label: 'photo.filter.vintage',
    adjust: { temperature: 22, fade: 34, saturation: -22, contrast: -8, vignette: 28, grain: 12 },
    split: { shadows: [0.04, 0.0, -0.05], highlights: [0.05, 0.03, -0.04] },
  },
  { id: 'fade', label: 'photo.filter.fade', adjust: { fade: 46, contrast: -16, saturation: -14, highlights: -10, exposure: 4 } },
  {
    id: 'film',
    label: 'photo.filter.film',
    adjust: { contrast: 10, fade: 16, grain: 26, temperature: 6, saturation: -6 },
    split: { shadows: [-0.03, 0.02, 0.05], highlights: [0.05, 0.02, -0.03] },
  },
  { id: 'dramatic', label: 'photo.filter.dramatic', adjust: { contrast: 36, highlights: -34, shadows: 22, saturation: -12, vignette: 32, exposure: -4, sharpen: 20 } },
  { id: 'matte', label: 'photo.filter.matte', adjust: { fade: 30, contrast: -20, saturation: -10, shadows: 16, vibrance: 8 } },
  { id: 'golden', label: 'photo.filter.golden', adjust: { temperature: 38, vibrance: 22, highlights: 10, exposure: 5, tint: 8 }, split: { shadows: [0.03, 0.0, -0.04], highlights: [0.06, 0.03, -0.05] } },
  {
    id: 'cinema',
    label: 'photo.filter.cinema',
    adjust: { contrast: 20, saturation: -8, fade: 8, vignette: 18 },
    split: { shadows: [-0.05, 0.03, 0.08], highlights: [0.08, 0.03, -0.06] },
  },
];

/** Resolved shader parameters (normalized to roughly −1…1). */
export interface RenderAdjust {
  exposure: number;
  brightness: number;
  contrast: number;
  highlights: number;
  shadows: number;
  saturation: number;
  vibrance: number;
  temperature: number;
  tint: number;
  hue: number; // radians
  fade: number;
  vignette: number;
  grain: number;
  sharpen: number;
  blur: number;
  split: SplitTone;
}

const NO_SPLIT: SplitTone = { shadows: [0, 0, 0], highlights: [0, 0, 0] };

/** Combine the user's sliders with the chosen filter preset (scaled by its strength). */
export function effectiveAdjust(adjust: Adjust, filter: { id: string | null; strength: number } | null): RenderAdjust {
  const preset = filter?.id ? FILTERS.find((f) => f.id === filter.id) : undefined;
  const k = preset ? filter!.strength / 100 : 0;
  const v = (key: AdjustKey) => {
    const [lo, hi] = ADJUST_RANGE[key];
    return Math.min(hi, Math.max(lo, adjust[key] + (preset?.adjust[key] ?? 0) * k));
  };
  const split = preset?.split ?? NO_SPLIT;
  // saturation multiplies, so "Mono" stays mono even after boosting saturation
  const sat = (1 + adjust.saturation / 100) * (1 + ((preset?.adjust.saturation ?? 0) * k) / 100) - 1;
  return {
    exposure: v('exposure') / 100,
    brightness: v('brightness') / 100,
    contrast: v('contrast') / 100,
    highlights: v('highlights') / 100,
    shadows: v('shadows') / 100,
    saturation: Math.max(-1, Math.min(1.5, sat)),
    vibrance: v('vibrance') / 100,
    temperature: v('temperature') / 100,
    tint: v('tint') / 100,
    hue: (v('hue') * Math.PI) / 180,
    fade: v('fade') / 100,
    vignette: v('vignette') / 100,
    grain: v('grain') / 100,
    sharpen: v('sharpen') / 100,
    blur: v('blur') / 100,
    split: {
      shadows: split.shadows.map((x) => x * k) as SplitTone['shadows'],
      highlights: split.highlights.map((x) => x * k) as SplitTone['highlights'],
    },
  };
}

export const NEUTRAL_RENDER: RenderAdjust = effectiveAdjust(NEUTRAL_ADJUST, null);

export function isNeutral(a: Adjust) {
  return (Object.keys(a) as AdjustKey[]).every((k) => a[k] === 0);
}
