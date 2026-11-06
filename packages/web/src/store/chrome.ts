import { create } from 'zustand';
import type { Crumb } from '@peekaround/shared';

/** What the top bar should show for the current page. */
interface ChromeStore {
  crumbs: Crumb[];
  title: string;
  viewControls: boolean;
  /** the page honours the sort / order options (the timeline and albums have a fixed order) */
  viewSort: boolean;
  /** the page honours the type filter (favorites is a filter itself) */
  viewKind: boolean;
  /** the media grid on screen is narrower than 700 px: it uses the phone density levels */
  gridPhone: boolean;
  /** mobile drawer */
  navOpen: boolean;
  set(c: Partial<Omit<ChromeStore, 'set'>>): void;
}

export const useChrome = create<ChromeStore>((set) => ({
  crumbs: [],
  title: '',
  viewControls: false,
  viewSort: true,
  viewKind: true,
  gridPhone: false,
  navOpen: false,
  set: (c) => set(c),
}));
