import { create } from 'zustand';
import type { Crumb } from '@peekaround/shared';

/** What the top bar should show for the current page. */
interface ChromeStore {
  crumbs: Crumb[];
  title: string;
  viewControls: boolean;
  /** mobile drawer */
  navOpen: boolean;
  set(c: Partial<Omit<ChromeStore, 'set'>>): void;
}

export const useChrome = create<ChromeStore>((set) => ({
  crumbs: [],
  title: '',
  viewControls: false,
  navOpen: false,
  set: (c) => set(c),
}));
