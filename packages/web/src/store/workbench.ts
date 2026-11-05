import { create } from 'zustand';
import type { MediaItem } from '@peekaround/shared';

/** Items handed from the gallery to the collage / slideshow tools. */
interface WorkbenchStore {
  items: MediaItem[];
  /** where results should be saved by default */
  target: { folderId: string; dir: string } | null;
  set(items: MediaItem[], target?: { folderId: string; dir: string } | null): void;
}

export const useWorkbench = create<WorkbenchStore>((set) => ({
  items: [],
  target: null,
  set: (items, target = null) => set({ items, target }),
}));
