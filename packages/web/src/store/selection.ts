import { create } from 'zustand';

interface SelectionStore {
  ids: Set<string>;
  anchor: string | null;
  toggle(id: string): void;
  set(ids: Iterable<string>, anchor?: string | null): void;
  add(ids: Iterable<string>): void;
  clear(): void;
}

export const useSelection = create<SelectionStore>((set, get) => ({
  ids: new Set(),
  anchor: null,
  toggle(id) {
    const n = new Set(get().ids);
    if (n.has(id)) n.delete(id);
    else n.add(id);
    set({ ids: n, anchor: id });
  },
  set(ids, anchor = null) {
    set({ ids: new Set(ids), anchor });
  },
  add(ids) {
    const n = new Set(get().ids);
    for (const i of ids) n.add(i);
    set({ ids: n });
  },
  clear() {
    if (get().ids.size) set({ ids: new Set(), anchor: null });
  },
}));
