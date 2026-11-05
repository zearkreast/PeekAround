import { create } from 'zustand';
import type { MediaItem } from '@peekaround/shared';

/**
 * Selected media. Items are kept by id (not just ids) so actions work for items whose
 * grid page has since been evicted / never rendered (select all, range drags).
 */
interface SelectionStore {
  ids: Set<string>;
  items: Map<string, MediaItem>;
  /** grid index of the last toggled item (shift / range selection anchor) */
  anchor: number | null;
  toggle(item: MediaItem, index?: number | null): void;
  set(items: Iterable<MediaItem>, anchor?: number | null): void;
  add(items: Iterable<MediaItem>): void;
  remove(ids: Iterable<string>): void;
  clear(): void;
  selected(): MediaItem[];
}

function make(items: Map<string, MediaItem>) {
  return { items, ids: new Set(items.keys()) };
}

export const useSelection = create<SelectionStore>((set, get) => ({
  ids: new Set(),
  items: new Map(),
  anchor: null,
  toggle(item, index = null) {
    const n = new Map(get().items);
    if (n.has(item.id)) n.delete(item.id);
    else n.set(item.id, item);
    set({ ...make(n), anchor: index });
  },
  set(items, anchor = null) {
    const n = new Map<string, MediaItem>();
    for (const i of items) n.set(i.id, i);
    set({ ...make(n), anchor });
  },
  add(items) {
    const n = new Map(get().items);
    for (const i of items) n.set(i.id, i);
    set(make(n));
  },
  remove(ids) {
    const n = new Map(get().items);
    for (const i of ids) n.delete(i);
    set(make(n));
  },
  clear() {
    if (get().ids.size) set({ ids: new Set(), items: new Map(), anchor: null });
  },
  selected() {
    return [...get().items.values()];
  },
}));

/** true while any item is selected (selection mode) */
export const useSelecting = () => useSelection((s) => s.ids.size > 0);
