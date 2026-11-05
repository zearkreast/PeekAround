import { create } from 'zustand';
import type { GridItems } from '@/features/gallery/grid/source';

/** What the gallery currently shows — used by drop targets, the viewer and the toolbar. */
interface LocationStore {
  target: { folderId: string; dir: string } | null;
  targetLabel: string;
  /** random-access list of the media in display order (viewer navigation, select all) */
  source: GridItems | null;
  setView(v: Partial<Pick<LocationStore, 'target' | 'targetLabel' | 'source'>>): void;
}

export const useLocation = create<LocationStore>((set) => ({
  target: null,
  targetLabel: '',
  source: null,
  setView: (v) => set(v),
}));
