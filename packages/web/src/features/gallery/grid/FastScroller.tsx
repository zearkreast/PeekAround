import { useEffect, useLayoutEffect, useRef, type RefObject } from 'react';
import { ChevronsUpDown } from 'lucide-react';
import { useT } from '@/i18n';

export interface FastScrollerTick {
  label: string;
  /** grid y of the tick */
  y: number;
}

/**
 * Google-Photos-style fast scroller for the page scroller: appears while scrolling,
 * draggable thumb (touch + mouse), date bubble and year ticks while dragging.
 * Everything per frame is direct DOM writes — no React state.
 */
export function FastScroller({
  scrollRef,
  marginRef,
  labelAt,
  ticks,
  onDrag,
  bottomInset = 0,
}: {
  scrollRef: RefObject<HTMLElement | null>;
  /** offset of the grid inside the scroller */
  marginRef: RefObject<number>;
  /** label for a grid y coordinate */
  labelAt: (gridY: number) => string;
  ticks: FastScrollerTick[];
  onDrag?: (dragging: boolean) => void;
  bottomInset?: number;
}) {
  const t = useT();
  const root = useRef<HTMLDivElement>(null);
  const thumb = useRef<HTMLDivElement>(null);
  const bubble = useRef<HTMLDivElement>(null);
  const tickBox = useRef<HTMLDivElement>(null);
  const st = useRef({ drag: false, off: 0, hide: 0 as unknown as ReturnType<typeof setTimeout>, trackH: 0, top: 0 });
  const labelRef = useRef(labelAt);
  labelRef.current = labelAt;
  const onDragRef = useRef(onDrag);
  onDragRef.current = onDrag;
  const THUMB = 46;

  // position over the scroller's right edge
  useLayoutEffect(() => {
    const sc = scrollRef.current;
    const el = root.current;
    if (!sc || !el) return;
    const place = () => {
      const r = sc.getBoundingClientRect();
      st.current.top = r.top + 8;
      st.current.trackH = Math.max(0, r.height - 16 - bottomInset);
      el.style.top = `${st.current.top}px`;
      el.style.height = `${st.current.trackH}px`;
      el.style.right = `${Math.max(0, window.innerWidth - r.right)}px`;
    };
    place();
    const ro = new ResizeObserver(place);
    ro.observe(sc);
    window.addEventListener('resize', place);
    return () => {
      ro.disconnect();
      window.removeEventListener('resize', place);
    };
  }, [scrollRef, bottomInset]);

  useEffect(() => {
    const sc = scrollRef.current;
    const el = root.current;
    if (!sc || !el) return;
    sc.dataset.fastScroll = '1';
    const sync = () => {
      const s = st.current;
      const max = sc.scrollHeight - sc.clientHeight;
      if (max < sc.clientHeight * 2) {
        delete el.dataset.on;
        return;
      }
      if (!s.drag) {
        const y = (sc.scrollTop / max) * (s.trackH - THUMB);
        thumb.current!.style.transform = `translateY(${y}px)`;
      }
      el.dataset.on = '1';
      clearTimeout(s.hide);
      if (!s.drag) s.hide = setTimeout(() => delete el.dataset.on, 1600);
    };
    sc.addEventListener('scroll', sync, { passive: true });
    return () => {
      sc.removeEventListener('scroll', sync);
      delete sc.dataset.fastScroll;
      clearTimeout(st.current.hide);
    };
  }, [scrollRef]);

  // year ticks (rendered once per layout, shown while dragging)
  useEffect(() => {
    const box = tickBox.current;
    const sc = scrollRef.current;
    if (!box || !sc) return;
    box.textContent = '';
    const max = sc.scrollHeight - sc.clientHeight;
    if (max <= 0) return;
    let lastY = -100;
    for (const tk of ticks) {
      const frac = Math.min(1, (tk.y + (marginRef.current ?? 0)) / max);
      const y = frac * (st.current.trackH - THUMB) + THUMB / 2;
      if (y - lastY < 22) continue;
      lastY = y;
      const d = document.createElement('div');
      d.className = 'fast-tick';
      d.style.top = `${y}px`;
      d.textContent = tk.label;
      box.appendChild(d);
    }
  }, [ticks, scrollRef, marginRef]);

  const move = (clientY: number) => {
    const sc = scrollRef.current;
    if (!sc) return;
    const s = st.current;
    const ty = Math.max(0, Math.min(s.trackH - THUMB, clientY - s.top - s.off));
    thumb.current!.style.transform = `translateY(${ty}px)`;
    const max = sc.scrollHeight - sc.clientHeight;
    const top = (ty / Math.max(1, s.trackH - THUMB)) * max;
    sc.scrollTop = top;
    const label = labelRef.current(top + sc.clientHeight * 0.15 - (marginRef.current ?? 0));
    if (bubble.current) {
      bubble.current.textContent = label;
      bubble.current.style.visibility = label ? 'visible' : 'hidden';
    }
  };

  return (
    <div ref={root} className="fast-scroller" data-fast-scroller aria-hidden>
      <div ref={tickBox} />
      <div
        ref={thumb}
        className="fast-thumb"
        data-fast-thumb
        title={t('mobile.fastScroll')}
        onPointerDown={(e) => {
          e.preventDefault();
          e.stopPropagation();
          const s = st.current;
          const r = thumb.current!.getBoundingClientRect();
          s.off = e.clientY - r.top;
          s.drag = true;
          clearTimeout(s.hide);
          root.current!.dataset.drag = '1';
          root.current!.dataset.on = '1';
          (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
          onDragRef.current?.(true);
          move(e.clientY);
        }}
        onPointerMove={(e) => {
          if (st.current.drag) move(e.clientY);
        }}
        onPointerUp={() => {
          const s = st.current;
          if (!s.drag) return;
          s.drag = false;
          delete root.current!.dataset.drag;
          onDragRef.current?.(false);
          s.hide = setTimeout(() => root.current && delete root.current.dataset.on, 1600);
        }}
        onPointerCancel={() => {
          st.current.drag = false;
          delete root.current!.dataset.drag;
          onDragRef.current?.(false);
        }}
      >
        <ChevronsUpDown className="size-4" />
        <div ref={bubble} className="fast-bubble" />
      </div>
    </div>
  );
}
