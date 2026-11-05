import { create } from 'zustand';

export interface UploadEntry {
  id: number;
  name: string;
  size: number;
  loaded: number;
  status: 'queued' | 'uploading' | 'processing' | 'done' | 'error' | 'canceled';
  error?: string;
}

interface UploadStore {
  entries: UploadEntry[];
  open: boolean;
  add(e: Omit<UploadEntry, 'id' | 'loaded' | 'status'>[]): number[];
  patch(id: number, p: Partial<UploadEntry>): void;
  clearDone(): void;
  setOpen(o: boolean): void;
}

let seq = 1;

export const useUploads = create<UploadStore>((set, get) => ({
  entries: [],
  open: false,
  add(list) {
    const ids: number[] = [];
    const added = list.map((e) => {
      const id = seq++;
      ids.push(id);
      return { ...e, id, loaded: 0, status: 'queued' as const };
    });
    set({ entries: [...get().entries.filter((x) => x.status !== 'done' && x.status !== 'canceled'), ...added], open: true });
    return ids;
  },
  patch(id, p) {
    set({ entries: get().entries.map((e) => (e.id === id ? { ...e, ...p } : e)) });
  },
  clearDone() {
    set({ entries: get().entries.filter((e) => e.status === 'queued' || e.status === 'uploading' || e.status === 'processing') });
  },
  setOpen: (open) => set({ open }),
}));

/** Pending interactive question (zip prompt / folder link prompt / target picker). */
export type Prompt =
  | { kind: 'zip'; names: string[]; resolve: (r: { mode: 'extract' | 'temp' | 'keep'; remember: boolean } | null) => void }
  | { kind: 'link'; names: string[]; resolve: (r: 'link' | 'copy' | null) => void }
  | { kind: 'target'; resolve: (r: { folderId: string; dir: string; label: string } | null) => void };

interface PromptStore {
  prompt: Prompt | null;
  ask<T>(p: Omit<Prompt, 'resolve'> & { kind: Prompt['kind'] }): Promise<T>;
  done(): void;
}

export const usePrompt = create<PromptStore>((set) => ({
  prompt: null,
  ask(p) {
    return new Promise((resolve) => set({ prompt: { ...(p as object), resolve } as Prompt }));
  },
  done: () => set({ prompt: null }),
}));
