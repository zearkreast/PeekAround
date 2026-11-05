import { create } from 'zustand';

interface ScanStore {
  scanning: boolean;
  done: number;
  total: number;
  set(s: { scanning: boolean; done: number; total: number }): void;
}

export const useScan = create<ScanStore>((set) => ({
  scanning: false,
  done: 0,
  total: 0,
  set: (s) => set({ scanning: s.scanning, done: s.done, total: s.total }),
}));
