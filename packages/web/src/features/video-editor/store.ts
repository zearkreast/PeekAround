import { create } from 'zustand';
import { baseSize, initialState, remapTexts, type EditState, type Size } from './model';

export type Tool = 'trim' | 'crop' | 'adjust' | 'text' | 'audio' | 'export';

type Patch = Partial<EditState> | ((s: EditState) => Partial<EditState>);

interface EditorStore {
  id: string | null;
  src: Size;
  duration: number;
  hasAudio: boolean;
  s: EditState;
  tool: Tool;
  selectedText: string | null;
  past: EditState[];
  future: EditState[];
  /** last state recorded in history */
  snap: EditState;
  init(id: string, src: Size, duration: number, hasAudio: boolean): void;
  set(p: Patch): void;
  setTool(t: Tool): void;
  select(id: string | null): void;
  undo(): void;
  redo(): void;
  reset(): void;
}

const HISTORY = 80;
let timer: ReturnType<typeof setTimeout> | null = null;

/** Record the current state in history (coalesces rapid changes such as slider drags). */
function checkpoint() {
  if (timer) {
    clearTimeout(timer);
    timer = null;
  }
  const st = useEditor.getState();
  if (st.s === st.snap) return;
  useEditor.setState({ past: [...st.past.slice(-HISTORY), st.snap], future: [], snap: st.s });
}

export const useEditor = create<EditorStore>((set, get) => ({
  id: null,
  src: { w: 1, h: 1 },
  duration: 1,
  hasAudio: false,
  s: initialState(1),
  tool: 'trim',
  selectedText: null,
  past: [],
  future: [],
  snap: initialState(1),

  init(id, src, duration, hasAudio) {
    if (timer) clearTimeout(timer);
    const s = initialState(duration);
    set({ id, src, duration, hasAudio, s, snap: s, past: [], future: [], tool: 'trim', selectedText: null });
  },

  set(p) {
    const { s, src } = get();
    const patch = typeof p === 'function' ? p(s) : p;
    let next: EditState = { ...s, ...patch };
    // keep text layers attached to the frame when crop / rotation change the design space
    if (('crop' in patch || 'rotate' in patch) && !('texts' in patch) && next.texts.length) {
      const from = baseSize(src, s.crop, s.rotate);
      const to = baseSize(src, next.crop, next.rotate);
      next = { ...next, texts: remapTexts(next.texts, from, to) };
    }
    set({ s: next });
    if (timer) clearTimeout(timer);
    timer = setTimeout(checkpoint, 400);
  },

  setTool(tool) {
    set({ tool });
  },

  select(selectedText) {
    set({ selectedText });
  },

  undo() {
    checkpoint();
    const { past, s, future } = get();
    if (!past.length) return;
    const prev = past[past.length - 1];
    set({ s: prev, snap: prev, past: past.slice(0, -1), future: [s, ...future] });
  },

  redo() {
    checkpoint();
    const { past, s, future } = get();
    if (!future.length) return;
    const next = future[0];
    set({ s: next, snap: next, past: [...past, s], future: future.slice(1) });
  },

  reset() {
    const { duration } = get();
    get().set(initialState(duration));
    set({ selectedText: null });
  },
}));

/** Transient playback state (kept out of the edit state so the timeline can update at 60 fps cheaply). */
interface PlayheadStore {
  /** current position in source seconds */
  time: number;
  playing: boolean;
  /** true while the user drags the playhead / a trim handle (transcoded sources show the filmstrip frame) */
  scrubbing: boolean;
}

export const usePlayhead = create<PlayheadStore>(() => ({ time: 0, playing: false, scrubbing: false }));
