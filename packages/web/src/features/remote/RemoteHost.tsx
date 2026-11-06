import { useEffect } from 'react';
import { create } from 'zustand';
import { MegaDialog } from './MegaDialog';
import { findMegaUrl } from './mega';
// registers the "copy to this computer" media action for MEGA items
import './RemoteFolders';

export interface RemoteShowOptions {
  /** preselect "watch without downloading" (add library flow) or "copy to this computer" */
  intent?: 'stream' | 'copy';
  /** stream into this library (default: a new library named after the link) */
  libraryId?: string;
}

interface RemoteUi {
  open: boolean;
  /** link to inspect right away (null → ask for one) */
  url: string | null;
  opts: RemoteShowOptions;
  /** bumps on every show() so the same link can be opened twice */
  seq: number;
  show(url?: string | null, opts?: RemoteShowOptions): void;
  close(): void;
}

export const useRemote = create<RemoteUi>((set) => ({
  open: false,
  url: null,
  opts: {},
  seq: 0,
  show: (url = null, opts = {}) => set((s) => ({ open: true, url, opts, seq: s.seq + 1 })),
  close: () => set({ open: false }),
}));

/** Open the MEGA dialog when `text` contains a MEGA link. Returns true when it did. */
export function openMegaFrom(text: string | null | undefined): boolean {
  const url = findMegaUrl(text);
  if (url) useRemote.getState().show(url);
  return !!url;
}

function editable(el: EventTarget | null): boolean {
  const e = el as HTMLElement | null;
  if (!e || !e.tagName) return false;
  return /^(input|textarea|select)$/i.test(e.tagName) || e.isContentEditable;
}

/**
 * MEGA links, app-wide: Ctrl/Cmd+V (or a mobile paste) of a mega.nz link anywhere outside a text
 * field opens the import dialog. (Dropped URLs: DropZone; Android share: ShareHandler.)
 */
export function RemoteHost() {
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      if (editable(e.target) || useRemote.getState().open) return;
      const text = e.clipboardData?.getData('text/plain') || e.clipboardData?.getData('text/uri-list') || '';
      if (openMegaFrom(text)) e.preventDefault();
    };
    window.addEventListener('paste', onPaste);
    return () => window.removeEventListener('paste', onPaste);
  }, []);
  return <MegaDialog />;
}
