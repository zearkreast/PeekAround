/**
 * Back-stack registry — one place that knows what "back" should close next.
 *
 *   const off = pushBack(() => close(), { prio: BACK.modal, label: 'modal' });   // imperative
 *   useBackHandler(open, close, BACK.sheet, 'sheet');                             // hook
 *   handleBack()  → closes the top entry (highest prio, then most recent); false when empty
 *   backDepth()   → number of entries (+1 when the in-app history can go back)
 *
 * Android: backend/android/back.ts mirrors `canHandleBack()` to the native
 * OnBackPressedCallback (enabled flag) and calls `performBack()` when the system back fires;
 * at the root the callback is disabled so Android exits / moves the task to the back.
 * Tests / the perf harness can drive it through `window.__peekBack`.
 */
import { useEffect, useRef } from 'react';

export const BACK = {
  drawer: 10,
  selection: 20,
  viewer: 30,
  panel: 35,
  sheet: 40,
  modal: 50,
} as const;

interface Entry {
  seq: number;
  prio: number;
  label: string;
  close: () => void;
}

let seq = 0;
const entries: Entry[] = [];
const listeners = new Set<() => void>();

function emit() {
  for (const l of [...listeners]) l();
}

export function pushBack(close: () => void, opts: { prio?: number; label?: string } = {}): () => void {
  const e: Entry = { seq: ++seq, prio: opts.prio ?? 0, label: opts.label ?? 'entry', close };
  entries.push(e);
  emit();
  return () => {
    const i = entries.indexOf(e);
    if (i >= 0) {
      entries.splice(i, 1);
      emit();
    }
  };
}

function top(): Entry | undefined {
  let best: Entry | undefined;
  for (const e of entries) if (!best || e.prio > best.prio || (e.prio === best.prio && e.seq > best.seq)) best = e;
  return best;
}

/** Close the top-most entry. Returns false when there is nothing to close. */
export function handleBack(): boolean {
  const e = top();
  if (!e) return false;
  const i = entries.indexOf(e);
  if (i >= 0) entries.splice(i, 1);
  emit();
  e.close();
  return true;
}

/** in-app history (react-router stores an index in history.state) */
export function canGoBackInHistory(): boolean {
  const idx = (typeof history !== 'undefined' && (history.state as { idx?: number } | null)?.idx) || 0;
  return idx > 0;
}

export function backDepth(): number {
  return entries.length + (canGoBackInHistory() ? 1 : 0);
}

export function canHandleBack(): boolean {
  return backDepth() > 0;
}

/** System back: close an overlay, else go back in history. Returns false at the root. */
export function performBack(): boolean {
  if (handleBack()) return true;
  if (canGoBackInHistory()) {
    history.back();
    return true;
  }
  return false;
}

export function backLabels(): string[] {
  return [...entries].sort((a, b) => b.prio - a.prio || b.seq - a.seq).map((e) => e.label);
}

export function onBackStackChange(fn: () => void): () => void {
  listeners.add(fn);
  return () => void listeners.delete(fn);
}

/** Notify listeners that the history position changed (Layout calls this on navigation). */
export function notifyHistoryChange() {
  emit();
}

/** Register `close` while `active`. The latest `close` is always used. */
export function useBackHandler(active: boolean, close: () => void, prio: number = BACK.modal, label = 'overlay') {
  const ref = useRef(close);
  ref.current = close;
  useEffect(() => {
    if (!active) return;
    return pushBack(() => ref.current(), { prio, label });
  }, [active, prio, label]);
}

declare global {
  interface Window {
    __peekBack?: { depth: () => number; back: () => boolean; labels: () => string[] };
  }
}
if (typeof window !== 'undefined') window.__peekBack = { depth: backDepth, back: performBack, labels: backLabels };

/** test helper */
export function _resetBackStack() {
  entries.length = 0;
  emit();
}
