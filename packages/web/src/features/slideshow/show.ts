import type { MediaItem, SlideshowExportSpec, SlideTransition } from '@peekaround/shared';

/** Pure slideshow model shared by the builder, the in-browser player and the export. */

export type Aspect = '16:9' | '9:16' | '1:1' | '4:5';
export type Res = 720 | 1080 | 2160;

export interface SlideItem {
  key: string;
  media: MediaItem;
  /** seconds (images); null = use the global duration */
  duration: number | null;
  /** video items: part to play */
  trim: { start: number; end: number } | null;
}

export interface SlideSettings {
  duration: number;
  transition: SlideTransition;
  transitionDuration: number;
  kenBurns: boolean;
  music: MediaItem | null;
  res: Res;
  aspect: Aspect;
  background: string;
  shuffle: boolean;
  loop: boolean;
  captions: boolean;
}

export const DEFAULT_SETTINGS: SlideSettings = {
  duration: 4,
  transition: 'fade',
  transitionDuration: 1,
  kenBurns: true,
  music: null,
  res: 1080,
  aspect: '16:9',
  background: '#000000',
  shuffle: false,
  loop: false,
  captions: false,
};

export const ASPECT_VALUES: Record<Aspect, number> = { '16:9': 16 / 9, '9:16': 9 / 16, '1:1': 1, '4:5': 4 / 5 };

/** Concrete in-browser transition styles (`random` and `kenburns` resolve to one of these). */
export type ShowTransition = 'none' | 'fade' | 'slide' | 'zoom' | 'blur' | 'flip' | 'circle';
export const RANDOM_POOL: ShowTransition[] = ['fade', 'slide', 'zoom', 'blur', 'flip', 'circle'];

const even = (n: number) => Math.round(n / 2) * 2;

/** Export size: `res` is the short side. */
export function outputDims(res: Res, aspect: Aspect): { w: number; h: number } {
  const r = ASPECT_VALUES[aspect];
  return r >= 1 ? { w: even(res * r), h: res } : { w: res, h: even(res / r) };
}

export function itemDuration(it: SlideItem, s: Pick<SlideSettings, 'duration'>): number {
  if (it.media.kind === 'video') {
    if (it.trim) return Math.max(0.2, it.trim.end - it.trim.start);
    return Math.max(0.2, it.duration ?? it.media.duration ?? s.duration);
  }
  return it.duration ?? s.duration;
}

export interface PlaySlide {
  key: string;
  media: MediaItem;
  /** seconds this slide is on screen (incl. the transition into the next one) */
  dur: number;
  trim: { start: number; end: number } | null;
  /** transition used when this slide comes in */
  transition: ShowTransition;
  /** Ken Burns move index, -1 = none */
  kb: number;
}

export function shuffled<T>(arr: T[], rnd: () => number = Math.random): T[] {
  const a = [...arr];
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(rnd() * (i + 1));
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}

export function kenBurnsOn(s: Pick<SlideSettings, 'kenBurns' | 'transition'>): boolean {
  return s.kenBurns || s.transition === 'kenburns';
}

/** Resolve the play order, durations, per-slide transitions (random picks) and Ken Burns moves. */
export function playlist(items: SlideItem[], s: SlideSettings, rnd: () => number = Math.random): PlaySlide[] {
  const order = s.shuffle ? shuffled(items, rnd) : items;
  const kb = kenBurnsOn(s);
  let move = 0;
  let prev: ShowTransition | null = null;
  return order.map((it, i) => {
    let transition: ShowTransition;
    if (i === 0 || s.transition === 'none') transition = i === 0 ? 'fade' : 'none';
    else if (s.transition === 'random') {
      const pool = RANDOM_POOL.filter((x) => x !== prev);
      transition = pool[Math.floor(rnd() * pool.length) % pool.length];
    } else if (s.transition === 'kenburns') transition = 'fade';
    else transition = s.transition as ShowTransition;
    if (i > 0) prev = transition;
    return {
      key: it.key,
      media: it.media,
      dur: itemDuration(it, s),
      trim: it.media.kind === 'video' ? (it.trim ?? null) : null,
      transition,
      kb: kb && it.media.kind === 'image' ? move++ % 4 : -1,
    };
  });
}

/** Length of the show: slides overlap by the transition duration (same as the export). */
export function showLength(slides: Pick<PlaySlide, 'dur'>[], td: number): number {
  if (!slides.length) return 0;
  const total = slides.reduce((a, x) => a + x.dur, 0);
  const cuts = slides.slice(1).reduce((a, x, i) => a + Math.min(td, slides[i].dur / 2, x.dur / 2), 0);
  return total - cuts;
}

/** Effective cross-fade length into slide i (clamped like the server does). */
export function cutLength(slides: Pick<PlaySlide, 'dur' | 'transition'>[], i: number, td: number): number {
  if (i <= 0 || slides[i].transition === 'none') return 0;
  return Math.min(td, slides[i - 1].dur / 2, slides[i].dur / 2);
}

export function buildExportSpec(
  items: SlideItem[],
  s: SlideSettings,
  opts: { target: { folderId: string; dir: string } | null; name: string; rnd?: () => number },
): SlideshowExportSpec {
  const order = s.shuffle ? shuffled(items, opts.rnd) : items;
  const dims = outputDims(s.res, s.aspect);
  return {
    items: order.map((it) => ({
      id: it.media.id,
      ...(it.media.kind === 'image' && it.duration ? { duration: it.duration } : {}),
      ...(it.media.kind === 'video' && it.trim ? { trim: it.trim } : {}),
      ...(it.media.kind === 'video' && !it.trim && it.duration ? { duration: it.duration } : {}),
    })),
    duration: s.duration,
    transition: s.transition,
    transitionDuration: s.transition === 'none' ? 0 : s.transitionDuration,
    kenBurns: kenBurnsOn(s),
    audioId: s.music?.id ?? null,
    width: dims.w,
    height: dims.h,
    fps: 30,
    background: s.background,
    ...(opts.target ? { target: opts.target } : {}),
    ...(opts.name.trim() ? { name: opts.name.trim() } : {}),
  };
}

/** Ken Burns moves (alternating zoom-in / zoom-out with different drift), like the exporter. */
export const KB_MOVES: { s: [number, number]; x: [number, number]; y: [number, number] }[] = [
  { s: [1, 1.15], x: [2.2, -2.2], y: [0, 0] },
  { s: [1.15, 1], x: [-2.2, 2.2], y: [1.2, -1.2] },
  { s: [1, 1.15], x: [0, 0], y: [-2.2, 2.2] },
  { s: [1.15, 1], x: [2.2, -2.2], y: [2.2, -2.2] },
];

export function kbAt(move: number, p: number): { scale: number; x: number; y: number } {
  const m = KB_MOVES[move % KB_MOVES.length];
  const t = Math.max(0, Math.min(1, p));
  const lerp = (a: [number, number]) => a[0] + (a[1] - a[0]) * t;
  return { scale: lerp(m.s), x: lerp(m.x), y: lerp(m.y) };
}
