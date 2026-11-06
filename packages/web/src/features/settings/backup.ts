import type { Settings } from '@peekaround/shared';
import { DEFAULT_SETTINGS as SERVER_DEFAULTS } from '@peekaround/shared';
import { DEFAULT_PREFS, type Prefs } from '@/store/prefs';
import { DEFAULT_SETTINGS as SLIDE_DEFAULTS, type SlideSettings } from '@/features/slideshow/show';

/** Settings → about → backup: one JSON file with per-device prefs, slideshow defaults and server settings. */
export interface SettingsBackup {
  app: 'peekaround';
  kind: 'settings';
  version: 1;
  exportedAt: string;
  prefs: Partial<Prefs>;
  slideshow: Partial<SlideSettings>;
  /** server settings (never hostBrowseRoots: those are the server admin's) */
  server?: Partial<Settings>;
}

/** allowed values of the enum-like prefs (anything else in a backup is dropped) */
const PREF_ENUMS: Partial<Record<keyof Prefs, readonly string[]>> = {
  lang: ['tr', 'en'],
  theme: ['dark', 'light', 'system'],
  motion: ['full', 'reduced', 'off'],
  gridMode: ['justified', 'square'],
  sort: ['takenAt', 'name', 'mtime', 'size', 'kind'],
  order: ['asc', 'desc'],
  kind: ['all', 'image', 'video', 'favorites'],
  groupBy: ['auto', 'day', 'month'],
  viewerBg: ['blur', 'black', 'tint'],
  infoDefault: ['remember', 'open', 'closed'],
  dateFormat: ['locale', 'iso'],
  hourCycle: ['auto', 'h23', 'h12'],
  weekStart: ['mon', 'sun', 'sat'],
};

/** numeric prefs: [min, max] */
const PREF_RANGES: Partial<Record<keyof Prefs, [number, number]>> = {
  rowHeight: [100, 480],
  gridZoomMobile: [0, 3],
  gridZoomDesktop: [0, 3],
  zoomStep: [1.1, 3],
  dblZoom: [1.5, 8],
  loopShortSec: [0, 600],
  videoSpeed: [0.25, 2],
  videoVolume: [0, 1],
};

const SLIDE_KEYS: (keyof SlideSettings)[] = ['duration', 'transition', 'transitionDuration', 'kenBurns', 'res', 'aspect', 'background', 'shuffle', 'loop', 'captions'];
const SLIDE_TRANSITIONS = ['none', 'fade', 'slide', 'zoom', 'kenburns', 'blur', 'flip', 'circle', 'random'];

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);

/** keep only known prefs whose type (and value set / range) matches the defaults */
export function sanitizePrefs(raw: unknown): Partial<Prefs> {
  if (!isObj(raw)) return {};
  const out: Record<string, unknown> = {};
  for (const [k, def] of Object.entries(DEFAULT_PREFS) as [keyof Prefs, unknown][]) {
    const v = raw[k];
    if (v === undefined || typeof v !== typeof def) continue;
    if (typeof v === 'number') {
      if (!Number.isFinite(v)) continue;
      const r = PREF_RANGES[k];
      out[k] = r ? Math.max(r[0], Math.min(r[1], v)) : v;
    } else if (typeof v === 'string') {
      const allowed = PREF_ENUMS[k];
      if (allowed && !allowed.includes(v)) continue;
      if (k === 'accent' && !/^#[0-9a-f]{6}$/i.test(v)) continue;
      out[k] = v;
    } else out[k] = v;
  }
  return out as Partial<Prefs>;
}

export function sanitizeSlideshow(raw: unknown): Partial<SlideSettings> {
  if (!isObj(raw)) return {};
  const out: Record<string, unknown> = {};
  for (const k of SLIDE_KEYS) {
    const v = raw[k];
    if (v === undefined || typeof v !== typeof SLIDE_DEFAULTS[k]) continue;
    if (k === 'transition' && !SLIDE_TRANSITIONS.includes(v as string)) continue;
    if (typeof v === 'number' && !Number.isFinite(v)) continue;
    out[k] = v;
  }
  if (typeof out.duration === 'number') out.duration = Math.max(0.5, Math.min(60, out.duration));
  if (typeof out.transitionDuration === 'number') out.transitionDuration = Math.max(0, Math.min(5, out.transitionDuration));
  return out as Partial<SlideSettings>;
}

/** server keys a backup may carry (the server validates each one again on PATCH) */
export function sanitizeServer(raw: unknown): Partial<Settings> {
  if (!isObj(raw)) return {};
  const out: Record<string, unknown> = {};
  for (const [k, def] of Object.entries(SERVER_DEFAULTS)) {
    if (k === 'hostBrowseRoots') continue;
    const v = raw[k];
    if (v !== undefined && typeof v === typeof def) out[k] = v;
  }
  return out as Partial<Settings>;
}

export function makeBackup(prefs: Prefs, slideshow: SlideSettings, server: Settings | undefined, now = new Date()): SettingsBackup {
  return {
    app: 'peekaround',
    kind: 'settings',
    version: 1,
    exportedAt: now.toISOString(),
    prefs: sanitizePrefs(prefs),
    slideshow: sanitizeSlideshow(slideshow),
    server: server ? sanitizeServer(server) : undefined,
  };
}

/** parse a backup file; null when it is not one */
export function parseBackup(text: string): SettingsBackup | null {
  let j: unknown;
  try {
    j = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isObj(j) || j.app !== 'peekaround' || j.kind !== 'settings' || j.version !== 1) return null;
  return {
    app: 'peekaround',
    kind: 'settings',
    version: 1,
    exportedAt: typeof j.exportedAt === 'string' ? j.exportedAt : '',
    prefs: sanitizePrefs(j.prefs),
    slideshow: sanitizeSlideshow(j.slideshow),
    server: j.server === undefined ? undefined : sanitizeServer(j.server),
  };
}

/** server settings to PATCH for "reset to defaults" (the home time zone and host roots stay) */
export function serverResetPatch(): Partial<Settings> {
  const { hostBrowseRoots: _r, homeTimeZone: _tz, ...rest } = SERVER_DEFAULTS;
  return rest;
}
