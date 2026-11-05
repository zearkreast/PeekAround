import { create } from 'zustand';
import type { MediaItem } from '@peekaround/shared';

/** What the gallery currently shows — used by drop targets, the viewer and the toolbar. */
interface LocationStore {
  target: { folderId: string; dir: string } | null;
  targetLabel: string;
  /** flat list of loaded media in display order (viewer navigation, range selection) */
  items: MediaItem[];
  setView(v: { target: LocationStore['target']; targetLabel: string; items: MediaItem[] }): void;
}

export const useLocation = create<LocationStore>((set) => ({
  target: null,
  targetLabel: '',
  items: [],
  setView: (v) => set(v),
}));
