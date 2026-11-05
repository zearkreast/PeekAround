import { create } from 'zustand';
import { applyChange, initHistory, redoHistory, undoHistory, type History } from './history';
import { emptyDoc, type EditDoc, type EditorTab } from './types';

export interface BrushSettings {
  color: string;
  size: number; // relative: % of the output's short side × 10 (1…100)
  opacity: number;
}

/** one-shot visual effect for rotate/flip buttons */
export interface SpinFx {
  kind: 'rotate' | 'flipH' | 'flipV';
  dir: 1 | -1;
  n: number;
}

interface EditorStore {
  h: History<EditDoc>;
  fx: SpinFx | null;
  /** doc as last saved (or as opened) — for the "unsaved changes" prompt */
  saved: EditDoc;
  tab: EditorTab;
  selectedId: string | null;
  compare: boolean;
  zoom: 'fit' | number;
  brush: BrushSettings;
  reset(): void;
  change(fn: (d: EditDoc) => EditDoc, key?: string): void;
  undo(): void;
  redo(): void;
  markSaved(): void;
  setTab(tab: EditorTab): void;
  select(id: string | null): void;
  setCompare(v: boolean): void;
  setZoom(z: 'fit' | number): void;
  setBrush(b: Partial<BrushSettings>): void;
  pulse(kind: SpinFx['kind'], dir?: 1 | -1): void;
}

export const useEditor = create<EditorStore>((set, get) => ({
  h: initHistory(emptyDoc()),
  fx: null,
  saved: emptyDoc(),
  tab: 'adjust',
  selectedId: null,
  compare: false,
  zoom: 'fit',
  brush: { color: '#ffffff', size: 12, opacity: 1 },
  reset() {
    const d = emptyDoc();
    set({ h: initHistory(d), saved: d, tab: 'adjust', selectedId: null, compare: false, zoom: 'fit' });
  },
  change(fn, key) {
    const h = get().h;
    set({ h: applyChange(h, fn(h.doc), key) });
  },
  undo() {
    const h = undoHistory(get().h);
    set({ h, selectedId: h.doc.layers.some((l) => l.id === get().selectedId) ? get().selectedId : null });
  },
  redo() {
    set({ h: redoHistory(get().h) });
  },
  markSaved() {
    set({ saved: get().h.doc });
  },
  setTab(tab) {
    set({ tab, zoom: tab === 'crop' ? 'fit' : get().zoom, selectedId: tab === 'crop' || tab === 'draw' ? null : get().selectedId });
  },
  select(selectedId) {
    set({ selectedId });
  },
  setCompare(compare) {
    set({ compare });
  },
  setZoom(zoom) {
    set({ zoom });
  },
  setBrush(b) {
    set({ brush: { ...get().brush, ...b } });
  },
  pulse(kind, dir = 1) {
    set({ fx: { kind, dir, n: (get().fx?.n ?? 0) + 1 } });
  },
}));

export const useDoc = () => useEditor((s) => s.h.doc);
