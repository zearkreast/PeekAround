import { create } from 'zustand';
import type { MediaItem } from '@peekaround/shared';
import { applyChange, initHistory, redoHistory, undoHistory, type History } from '@/features/photo-editor/history';
import { emptySlot, newDoc, type CollageDoc } from './model';

export type CollageTab = 'layout' | 'style' | 'photos' | 'text';

interface CollageStore {
  /** photo pool (not undoable) */
  photos: MediaItem[];
  h: History<CollageDoc>;
  /** signature of the workbench hand-over this collage was started from */
  sourceKey: string;
  target: { folderId: string; dir: string } | null;
  tab: CollageTab;
  cell: number | null;
  textId: string | null;
  /** natural image sizes, filled as thumbnails load */
  dims: Record<string, { w: number; h: number }>;
  start(photos: MediaItem[], target: { folderId: string; dir: string } | null, key: string): void;
  change(fn: (d: CollageDoc) => CollageDoc, key?: string): void;
  undo(): void;
  redo(): void;
  /** add to the pool; with `slot`, put the first item into that cell */
  addPhotos(items: MediaItem[], slot?: number): void;
  removePhoto(id: string): void;
  setTarget(t: { folderId: string; dir: string } | null): void;
  setTab(t: CollageTab): void;
  selectCell(i: number | null): void;
  selectText(id: string | null): void;
  setDims(id: string, w: number, h: number): void;
}

export const useCollage = create<CollageStore>((set, get) => ({
  photos: [],
  h: initHistory(newDoc([])),
  sourceKey: '',
  target: null,
  tab: 'layout',
  cell: null,
  textId: null,
  dims: {},
  start(photos, target, key) {
    const imgs = photos.filter((p) => p.kind === 'image');
    set({ photos: imgs, h: initHistory(newDoc(imgs.map((p) => p.id))), sourceKey: key, target, cell: null, textId: null, tab: 'layout' });
  },
  change(fn, key) {
    const h = get().h;
    set({ h: applyChange(h, fn(h.doc), key) });
  },
  undo() {
    set({ h: undoHistory(get().h) });
  },
  redo() {
    set({ h: redoHistory(get().h) });
  },
  addPhotos(items, slot) {
    const have = new Set(get().photos.map((p) => p.id));
    const fresh = items.filter((i) => i.kind === 'image' && !have.has(i.id));
    if (slot !== undefined && items[0]) {
      const id = items[0].id;
      if (fresh.length) set({ photos: [...get().photos, ...fresh] });
      get().change((d) => {
        const slots = [...d.slots];
        const prev = slots.findIndex((x, i) => i < d.count && x.id === id);
        if (prev >= 0 && prev !== slot) slots[prev] = { ...slots[slot] };
        slots[slot] = emptySlot(id);
        return { ...d, slots };
      });
      return;
    }
    if (!fresh.length) return;
    const before = get().photos.length;
    const photos = [...get().photos, ...fresh];
    set({ photos, target: get().target ?? { folderId: fresh[0].folderId, dir: fresh[0].dir } });
    get().change((d) => {
      const queue = fresh.map((f) => f.id);
      const slots = [...d.slots];
      if (before === 0) {
        for (let i = 0; i < slots.length && queue.length; i++) slots[i] = emptySlot(queue.shift()!);
        return { ...d, slots, count: Math.min(9, Math.max(2, fresh.length)) };
      }
      // new photos go into empty cells first, then the layout grows to show the rest
      for (let i = 0; i < d.count && queue.length; i++) if (!slots[i].id) slots[i] = emptySlot(queue.shift()!);
      let count = d.count;
      while (queue.length && count < 9) slots[count++] = emptySlot(queue.shift()!);
      return { ...d, slots, count };
    });
  },
  removePhoto(id) {
    set({ photos: get().photos.filter((p) => p.id !== id) });
    get().change((d) => ({ ...d, slots: d.slots.map((s) => (s.id === id ? emptySlot() : s)) }));
  },
  setTarget(target) {
    set({ target });
  },
  setTab(tab) {
    set({ tab });
  },
  selectCell(cell) {
    set({ cell, textId: cell !== null ? null : get().textId });
  },
  selectText(textId) {
    set({ textId, cell: textId ? null : get().cell });
  },
  setDims(id, w, h) {
    const d = get().dims[id];
    if (d && d.w === w && d.h === h) return;
    set({ dims: { ...get().dims, [id]: { w, h } } });
  },
}));

export const useCollageDoc = () => useCollage((s) => s.h.doc);
