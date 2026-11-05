import { create } from 'zustand';
import type { MediaItem } from '@peekaround/shared';
import { load, save } from '@/store/persist';
import { DEFAULT_SETTINGS, type SlideItem, type SlideSettings } from './show';

let seq = 0;
const key = (m: MediaItem) => `${m.id}-${(seq++).toString(36)}`;

export const toSlideItem = (m: MediaItem): SlideItem => ({ key: key(m), media: m, duration: null, trim: null });

interface SlideshowStore {
  items: SlideItem[];
  /** default save location (from the gallery hand-over) */
  target: { folderId: string; dir: string } | null;
  /** the workbench list the items were taken from (re-sync when it changes) */
  source: MediaItem[] | null;
  settings: SlideSettings;
  sync(items: MediaItem[], target: { folderId: string; dir: string } | null): void;
  setItems(items: SlideItem[]): void;
  add(media: MediaItem[]): void;
  remove(key: string): void;
  update(key: string, patch: Partial<Pick<SlideItem, 'duration' | 'trim'>>): void;
  setSettings(patch: Partial<SlideSettings>): void;
  setTarget(t: { folderId: string; dir: string } | null): void;
  clear(): void;
}

const STORE_KEY = 'peek.slideshow';

export const useSlideshow = create<SlideshowStore>((set, get) => ({
  items: [],
  target: null,
  source: null,
  settings: load(STORE_KEY, DEFAULT_SETTINGS),

  sync(items, target) {
    if (items === get().source) return;
    set({
      source: items,
      items: items.filter((m) => m.kind === 'image' || m.kind === 'video').map(toSlideItem),
      target: target ?? (items[0] ? { folderId: items[0].folderId, dir: items[0].dir } : get().target),
    });
  },
  setItems: (items) => set({ items }),
  add: (media) => set((s) => ({ items: [...s.items, ...media.filter((m) => m.kind === 'image' || m.kind === 'video').map(toSlideItem)] })),
  remove: (k) => set((s) => ({ items: s.items.filter((i) => i.key !== k) })),
  update: (k, patch) => set((s) => ({ items: s.items.map((i) => (i.key === k ? { ...i, ...patch } : i)) })),
  setSettings(patch) {
    const settings = { ...get().settings, ...patch };
    set({ settings });
    save(STORE_KEY, settings);
  },
  setTarget: (target) => set({ target }),
  clear: () => set({ items: [] }),
}));
