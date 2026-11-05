import { useCallback, useEffect, useRef, useState } from 'react';
import { useMotionValue, type MotionValue } from 'motion/react';
import { cutLength, type PlaySlide } from './show';

export interface ShowState {
  index: number;
  /** +1 forward, -1 backward (slide direction) */
  dir: number;
  playing: boolean;
  ended: boolean;
  /** show time in seconds — only advances while playing (drives Ken Burns, so pausing freezes motion) */
  clock: MotionValue<number>;
  /** clock value when the current slide started */
  startAt: number;
  /** 0..1 progress of the current slide */
  progress: MotionValue<number>;
  /** seconds left in the whole show (for the music fade-out) */
  remaining: MotionValue<number>;
  setPlaying(p: boolean): void;
  toggle(): void;
  next(): void;
  prev(): void;
  goTo(i: number): void;
  restart(): void;
}

/** Drives a slideshow: one rAF clock, auto-advance (slides overlap by the transition, like the export), loop and end. */
export function useShow(slides: PlaySlide[], td: number, opts: { autoplay: boolean; loop: boolean }): ShowState {
  const clock = useMotionValue(0);
  const progress = useMotionValue(0);
  const remaining = useMotionValue(0);
  const [index, setIndex] = useState(0);
  const [dir, setDir] = useState(1);
  const [playing, setPlayingState] = useState(opts.autoplay);
  const [ended, setEnded] = useState(false);
  const [startAt, setStartAt] = useState(0);
  const st = useRef({ index: 0, startAt: 0, playing: opts.autoplay, ended: false, loop: opts.loop });
  st.current.loop = opts.loop;
  const n = slides.length;

  const go = useCallback(
    (i: number, d: number) => {
      const s = clock.get();
      st.current.index = i;
      st.current.startAt = s;
      st.current.ended = false;
      setDir(d);
      setIndex(i);
      setStartAt(s);
      setEnded(false);
      progress.set(0);
    },
    [clock, progress],
  );

  const setPlaying = useCallback((p: boolean) => {
    st.current.playing = p;
    setPlayingState(p);
  }, []);

  const next = useCallback(() => {
    const i = st.current.index;
    if (i < n - 1) go(i + 1, 1);
    else if (st.current.loop && n > 1) go(0, 1);
    else {
      st.current.ended = true;
      setEnded(true);
      setPlaying(false);
    }
  }, [n, go, setPlaying]);

  const prev = useCallback(() => {
    const i = st.current.index;
    if (clock.get() - st.current.startAt > 1.5 || i === 0) go(i, -1);
    else go(i - 1, -1);
  }, [go, clock]);

  // keep the refs right if the list changes under us
  useEffect(() => {
    if (st.current.index >= n && n) go(0, 1);
  }, [n, go]);

  useEffect(() => {
    let raf = 0;
    let last = performance.now();
    const tick = (now: number) => {
      raf = requestAnimationFrame(tick);
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      const s = st.current;
      if (!n) return;
      const cur = slides[Math.min(s.index, n - 1)];
      if (s.playing && !s.ended) clock.set(clock.get() + dt);
      const elapsed = clock.get() - s.startAt;
      const nextIdx = s.index < n - 1 ? s.index + 1 : s.loop && n > 1 ? 0 : -1;
      const cut = nextIdx > 0 ? cutLength(slides, nextIdx, td) : nextIdx === 0 ? Math.min(td, cur.dur / 2, slides[0].dur / 2) : 0;
      const advanceAt = Math.max(0.2, cur.dur - cut);
      progress.set(Math.min(1, elapsed / advanceAt));
      let rest = Math.max(0, advanceAt - elapsed);
      for (let j = s.index + 1; j < n; j++) rest += slides[j].dur - (j < n - 1 ? cutLength(slides, j + 1, td) : 0);
      remaining.set(rest);
      if (s.playing && !s.ended && elapsed >= advanceAt) next();
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [slides, n, td, clock, progress, remaining, next]);

  const toggle = useCallback(() => {
    if (st.current.ended) {
      go(0, 1);
      setPlaying(true);
    } else setPlaying(!st.current.playing);
  }, [go, setPlaying]);

  const restart = useCallback(() => {
    go(0, 1);
    setPlaying(true);
  }, [go, setPlaying]);

  const goTo = useCallback((i: number) => go(Math.max(0, Math.min(n - 1, i)), i >= st.current.index ? 1 : -1), [go, n]);

  return { index: Math.min(index, Math.max(0, n - 1)), dir, playing, ended, clock, startAt, progress, remaining, setPlaying, toggle, next, prev, goTo, restart };
}
