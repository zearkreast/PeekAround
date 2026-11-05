/**
 * Tiny undo/redo helper shared by the photo editor and the collage.
 * Consecutive changes with the same `key` within COALESCE_MS merge into one step
 * (slider drags, typing), so history stays meaningful without commit callbacks.
 */
export interface History<T> {
  doc: T;
  past: T[];
  future: T[];
  lastKey: string | null;
  lastAt: number;
}

const LIMIT = 120;
const COALESCE_MS = 900;

export function initHistory<T>(doc: T): History<T> {
  return { doc, past: [], future: [], lastKey: null, lastAt: 0 };
}

export function applyChange<T>(h: History<T>, next: T, key?: string): History<T> {
  if (next === h.doc) return h;
  const now = performance.now();
  const merge = !!key && key === h.lastKey && now - h.lastAt < COALESCE_MS;
  return {
    doc: next,
    past: merge ? h.past : [...h.past.slice(-(LIMIT - 1)), h.doc],
    future: [],
    lastKey: key ?? null,
    lastAt: now,
  };
}

export function undoHistory<T>(h: History<T>): History<T> {
  if (!h.past.length) return h;
  return { doc: h.past[h.past.length - 1], past: h.past.slice(0, -1), future: [h.doc, ...h.future], lastKey: null, lastAt: 0 };
}

export function redoHistory<T>(h: History<T>): History<T> {
  if (!h.future.length) return h;
  return { doc: h.future[0], past: [...h.past, h.doc], future: h.future.slice(1), lastKey: null, lastAt: 0 };
}

/** Ctrl/Cmd+Z, Ctrl/Cmd+Shift+Z and Ctrl+Y — ignores typing in inputs. */
export function isUndoKey(e: KeyboardEvent): 'undo' | 'redo' | null {
  if (!(e.ctrlKey || e.metaKey) || e.altKey) return null;
  const k = e.key.toLowerCase();
  if (k === 'z') return e.shiftKey ? 'redo' : 'undo';
  if (k === 'y') return 'redo';
  return null;
}

export function isTyping(e: Event) {
  const el = e.target as HTMLElement | null;
  return !!el && (/^(input|textarea|select)$/i.test(el.tagName) || el.isContentEditable);
}
