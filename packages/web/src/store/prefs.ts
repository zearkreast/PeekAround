import { create } from 'zustand';
import type { KindFilter, SortDir, SortKey } from '@peekaround/shared';
import { load, save } from './persist';

export type Lang = 'tr' | 'en';
export type ThemePref = 'dark' | 'light' | 'system';
export type MotionPref = 'full' | 'reduced' | 'off';
export type GridMode = 'justified' | 'square';

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
}

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
};

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
