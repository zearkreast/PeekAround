import { create } from 'zustand';
import type { KindFilter, SortDir, SortKey } from '@peekaround/shared';
import { load, save } from './persist';

export type Lang = 'tr' | 'en';
export type ThemePref = 'dark' | 'light' | 'system';
export type MotionPref = 'full' | 'reduced' | 'off';
export type GridMode = 'justified' | 'square';
export type GroupByPref = 'auto' | 'day' | 'month';
export type ViewerBg = 'blur' | 'black' | 'tint';
export type InfoDefault = 'remember' | 'open' | 'closed';
export type DateFormatPref = 'locale' | 'iso';
export type HourCyclePref = 'auto' | 'h23' | 'h12';
export type WeekStart = 'mon' | 'sun' | 'sat';
/** JS getDay() number of each week start */
export const WEEK_START_DAY: Record<WeekStart, number> = { mon: 1, sun: 0, sat: 6 };

export interface Prefs {
  lang: Lang;
  theme: ThemePref;
  accent: string;
  motion: MotionPref;
  gridMode: GridMode;
  rowHeight: number;
  sort: SortKey;
  order: SortDir;
  kind: KindFilter;
  recursive: boolean;
  sidebarCollapsed: boolean;
  infoOpen: boolean;
  hoverPreview: boolean;
  /** grid density level 0 (big) … 3 (month overview); phones and desktop separately */
  gridZoomMobile: number;
  gridZoomDesktop: number;

  // ── gallery (settings page) ──
  /** day / month headers in date sorted grids */
  dateHeaders: boolean;
  /** group headers by day or month ('auto' = by zoom level) */
  groupBy: GroupByPref;
  /** file name caption on tiles */
  tileNames: boolean;
  /** duration badge on video / audio tiles */
  tileDuration: boolean;
  /** heart on favourite tiles */
  tileFavorites: boolean;
  /** ask before moving files to the trash (permanent deletes always ask) */
  confirmDelete: boolean;

  // ── viewer & video ──
  viewerBg: ViewerBg;
  /** desktop thumbnail strip under the picture */
  filmstrip: boolean;
  /** what the info panel does when the viewer opens */
  infoDefault: InfoDefault;
  /** keyboard / button zoom factor per step */
  zoomStep: number;
  /** double click / double tap zoom level */
  dblZoom: number;
  videoAutoplay: boolean;
  videoMuted: boolean;
  /** loop videos shorter than this many seconds (0 = never) */
  loopShortSec: number;
  videoSpeed: number;
  rememberVolume: boolean;
  /** last volume (rememberVolume) */
  videoVolume: number;

  // ── explore, language, privacy ──
  showMemories: boolean;
  weekStart: WeekStart;
  dateFormat: DateFormatPref;
  hourCycle: HourCyclePref;
  /** hide GPS coordinates / place in the info panel (one click reveals) */
  hideLocation: boolean;

  // ── library & sidebar (UI state, features/library) ──
  /** library / folder pages: 'all' = every item recursively, 'folders' = folder browsing; per library id */
  libraryView: Record<string, LibraryViewMode>;
  /** folder cards order on folder pages */
  folderSort: FolderSort;
  /** collapsed sidebar sections (albums, libraries, tools …) */
  navCollapsed: Record<string, boolean>;
}

export type LibraryViewMode = 'all' | 'folders';
export type FolderSort = 'name' | 'date' | 'count';

export const ACCENTS = ['#8b5cf6', '#ec4899', '#f97316', '#10b981', '#06b6d4', '#3b82f6', '#eab308', '#ef4444'];

const defaults: Prefs = {
  lang: (typeof navigator !== 'undefined' && navigator.language?.toLowerCase().startsWith('en') ? 'en' : 'tr') as Lang,
  theme: 'dark',
  accent: '#8b5cf6',
  motion: 'full',
  gridMode: 'justified',
  rowHeight: 220,
  sort: 'takenAt',
  order: 'desc',
  kind: 'all',
  recursive: false,
  sidebarCollapsed: false,
  infoOpen: false,
  hoverPreview: true,
  gridZoomMobile: 1,
  gridZoomDesktop: 1,
  dateHeaders: true,
  groupBy: 'auto',
  tileNames: false,
  tileDuration: true,
  tileFavorites: true,
  confirmDelete: true,
  viewerBg: 'blur',
  filmstrip: true,
  infoDefault: 'remember',
  zoomStep: 1.4,
  dblZoom: 2.5,
  videoAutoplay: true,
  videoMuted: false,
  loopShortSec: 0,
  videoSpeed: 1,
  rememberVolume: true,
  videoVolume: 1,
  showMemories: true,
  weekStart: 'mon',
  dateFormat: 'locale',
  hourCycle: 'auto',
  hideLocation: false,
  libraryView: {},
  folderSort: 'name',
  navCollapsed: {},
};

/** factory defaults (settings → reset / import) */
export const DEFAULT_PREFS: Readonly<Prefs> = defaults;

interface PrefsStore extends Prefs {
  set(p: Partial<Prefs>): void;
}

export const usePrefs = create<PrefsStore>((set, get) => ({
  ...load('peek.prefs', defaults),
  set(p) {
    set(p);
    const { set: _s, ...rest } = { ...get(), ...p };
    save('peek.prefs', rest);
  },
}));
