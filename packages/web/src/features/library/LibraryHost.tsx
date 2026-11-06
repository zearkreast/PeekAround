import { create } from 'zustand';
import type { Library } from '@peekaround/shared';
import { LibraryEditor } from '@/features/settings/LibraryEditor';
import { AlbumPicker } from './AlbumPicker';
import { ExclusionsDialog } from './ExclusionsDialog';
import './actions';
import './library.css';

/** library dialogs opened from anywhere (hub cards, library header, sidebar) */
export const useLibraryUi = create<{
  editing: Library | 'new' | null;
  exclusions: Library | null;
  edit(l: Library | 'new' | null): void;
  exclude(l: Library | null): void;
}>((set) => ({
  editing: null,
  exclusions: null,
  edit: (editing) => set({ editing }),
  exclude: (exclusions) => set({ exclusions }),
}));

/** Mount once (Layout): album picker, library editor + exclusions dialogs, library / album actions. */
export function LibraryHost() {
  const editing = useLibraryUi((s) => s.editing);
  const exclusions = useLibraryUi((s) => s.exclusions);
  const ui = useLibraryUi.getState();
  return (
    <>
      <AlbumPicker />
      <LibraryEditor value={editing} onClose={() => ui.edit(null)} />
      <ExclusionsDialog lib={exclusions} onClose={() => ui.exclude(null)} />
    </>
  );
}
