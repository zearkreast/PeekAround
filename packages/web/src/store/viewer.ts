import { create } from 'zustand';

/** id of the media whose tile ↔ viewer shared-element transition is active */
interface ViewerStore {
  activeId: string | null;
  setActive(id: string | null): void;
}

export const useViewer = create<ViewerStore>((set) => ({
  activeId: null,
  setActive: (activeId) => set({ activeId }),
}));
