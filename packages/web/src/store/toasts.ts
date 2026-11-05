import { create } from 'zustand';
import type { ReactNode } from 'react';

export interface Toast {
  id: number;
  kind: 'info' | 'success' | 'error' | 'progress';
  title: ReactNode;
  body?: ReactNode;
  progress?: number;
  action?: { label: string; run: () => void };
  duration?: number;
}

interface ToastStore {
  toasts: Toast[];
  push(t: Omit<Toast, 'id'>): number;
  update(id: number, t: Partial<Toast>): void;
  dismiss(id: number): void;
}

let seq = 1;

export const useToasts = create<ToastStore>((set, get) => ({
  toasts: [],
  push(t) {
    const id = seq++;
    set({ toasts: [...get().toasts.slice(-4), { ...t, id }] });
    const d = t.duration ?? (t.kind === 'error' ? 6000 : t.kind === 'progress' ? 0 : 3500);
    if (d) setTimeout(() => get().dismiss(id), d);
    return id;
  },
  update(id, t) {
    set({ toasts: get().toasts.map((x) => (x.id === id ? { ...x, ...t } : x)) });
    if (t.kind && t.kind !== 'progress') setTimeout(() => get().dismiss(id), t.duration ?? 3500);
  },
  dismiss(id) {
    set({ toasts: get().toasts.filter((x) => x.id !== id) });
  },
}));

export const toast = {
  info: (title: ReactNode, body?: ReactNode) => useToasts.getState().push({ kind: 'info', title, body }),
  success: (title: ReactNode, body?: ReactNode, action?: Toast['action']) => useToasts.getState().push({ kind: 'success', title, body, action }),
  error: (title: ReactNode, body?: ReactNode) => useToasts.getState().push({ kind: 'error', title, body }),
};
