import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react';
import { CheckCircle2 } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useLang, useT } from '@/i18n';
import { formatBytes, formatDayKey, formatLocal, formatMonthKey } from '@/lib/format';
import { usePrefs, type GridMode } from '@/store/prefs';
import { useSelecting, useSelection } from '@/store/selection';
import { ContextMenu } from '@/components/ui';
import { MediaMenu } from './actions';
import { MediaTile } from './MediaTile';
import { layoutGrid, nearestItem, rowAt, itemAt, type GridLayout, type GroupBy } from './grid/layout';
import { useSourceVersion, type GridItems } from './grid/source';
import { FastScroller, type FastScrollerTick } from './grid/FastScroller';

export interface MediaGridHandle {
  scrollToItem(id: string): void;
  scrollToIndex(i: number, align?: 'auto' | 'start' | 'center'): void;
  scrollToGroup(key: string): void;
}

declare global {
  interface Window {
    /** perf harness: last layout time (ms) and item count */
    __peekLayout?: { ms: number; total: number; rows: number };
  }
}

export const ZOOM_LEVELS = 4;

interface GridParams {
  target: number;
  square: boolean;
  gap: number;
  groupBy: GroupBy;
  headerH: number;
}

/** Density levels: 0 big … 3 month overview. Phones are sized by width, desktop by the row-height pref. */
export function gridParams(width: number, level: number, rowHeight: number, gridMode: GridMode): GridParams {
  const sq = gridMode === 'square';
  if (width < 700) {
    switch (level) {
      case 0:
        return { target: width / 2.05, square: sq, gap: 2, groupBy: 'day', headerH: 50 };
      case 2:
        return { target: width / 5.2, square: true, gap: 2, groupBy: 'day', headerH: 44 };
      case 3:
        return { target: width / 8.5, square: true, gap: 1, groupBy: 'month', headerH: 46 };
      default:
        return { target: Math.max(96, width / 3.3), square: sq, gap: 2, groupBy: 'day', headerH: 48 };
    }
  }
  switch (level) {
    case 0:
      return { target: rowHeight * 1.6, square: sq, gap: 6, groupBy: 'day', headerH: 62 };
    case 2:
      return { target: Math.max(72, rowHeight * 0.55), square: true, gap: 3, groupBy: 'day', headerH: 54 };
    case 3:
      return { target: 56, square: true, gap: 2, groupBy: 'month', headerH: 54 };
    default:
      return { target: rowHeight, square: sq, gap: 6, groupBy: 'day', headerH: 60 };
  }
}

const GroupHeader = memo(function GroupHeader({ y, h, label, place, count, index, phone }: { y: number; h: number; label: string; place: string | null; count: number; index: number; phone: boolean }) {
  const t = useT();
  return (
    <div className="grid-header group/h" style={{ top: y, height: h }}>
      <h3 className={phone ? 'shrink-0 text-[15px] font-semibold capitalize' : 'shrink-0 font-display text-lg font-semibold capitalize'}>{label}</h3>
      {place && <span className="min-w-0 truncate pb-px text-xs text-dim">{place}</span>}
      {!phone && <span className="pb-0.5 text-xs text-faint tabular-nums">{count}</span>}
      <button
        type="button"
        data-group-select={index}
        title={t('mobile.selectDay')}
        aria-label={t('mobile.selectDay')}
        className="group-select ml-auto grid size-9 shrink-0 place-items-center rounded-full text-faint transition-[opacity,color] hover:bg-surface-2 hover:text-accent"
      >
        <CheckCircle2 className="size-[18px]" />
      </button>
    </div>
  );
});

interface RangeDrag {
  anchor: number;
  adding: boolean;
  base: Map<string, MediaItem>;
  last: number;
  x: number;
  y: number;
  speed: number;
  raf: number;
}

export function MediaGrid({
  source,
  scrollRef,
  onOpen,
  groups = true,
  handleRef,
  fastScroller = true,
  zoomable = true,
  sortKey,
}: {
  source: GridItems;
  scrollRef: RefObject<HTMLElement | null>;
  onOpen: (item: MediaItem, index: number) => void;
  /** day/month headers when the source has day groups (sorted by date) */
  groups?: boolean;
  handleRef?: RefObject<MediaGridHandle | null>;
  fastScroller?: boolean;
  /** pinch / ctrl+wheel density */
  zoomable?: boolean;
  /** sort of the source (fast scroller labels for non-date sorts) */
  sortKey?: 'takenAt' | 'name' | 'mtime' | 'size' | 'kind';
}) {
  const t = useT();
  const lang = useLang();
  useSourceVersion(source);
  const rowHeight = usePrefs((s) => s.rowHeight);
  const gridMode = usePrefs((s) => s.gridMode);
  const levelMobile = usePrefs((s) => s.gridZoomMobile);
  const levelDesktop = usePrefs((s) => s.gridZoomDesktop);
  const hoverPreviewPref = usePrefs((s) => s.hoverPreview);
  const selecting = useSelecting();
  const gridRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [margin, setMargin] = useState(0);
  const marginRef = useRef(0);
  const [win, setWin] = useState<[number, number]>([0, -1]);
  const [fast, setFast] = useState(false);
  const fastRef = useRef(false);
  const [preview, setPreview] = useState(-1);
  const [menu, setMenu] = useState<{ at: { x: number; y: number }; items: MediaItem[]; index: number } | null>(null);
  const [lasso, setLasso] = useState<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
  const phone = width > 0 && width < 700;
  const level = Math.max(0, Math.min(ZOOM_LEVELS - 1, phone ? levelMobile : levelDesktop));

  useLayoutEffect(() => {
    const el = gridRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(Math.floor(el.clientWidth)));
    ro.observe(el);
    setWidth(Math.floor(el.clientWidth));
    return () => ro.disconnect();
  }, []);

  // offset of the grid inside the scroll container (content above it may change size)
  useLayoutEffect(() => {
    const sc = scrollRef.current;
    const el = gridRef.current;
    if (!sc || !el) return;
    const measure = () => {
      const m = el.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
      if (Math.abs(marginRef.current - m) > 0.5) {
        marginRef.current = m;
        setMargin(m);
      }
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (sc.firstElementChild) ro.observe(sc.firstElementChild);
    ro.observe(sc);
    return () => ro.disconnect();
  }, [scrollRef]);

  // ── exact layout of every item ──
  const params = useMemo(() => gridParams(width, level, rowHeight, gridMode), [width, level, rowHeight, gridMode]);
  const layoutVersion = source.layoutVersion;
  const layout: GridLayout = useMemo(() => {
    const t0 = performance.now();
    const l = layoutGrid(source.aspects, source.total, groups ? source.days : null, { ...params, width });
    window.__peekLayout = { ms: +(performance.now() - t0).toFixed(2), total: source.total, rows: l.rowCount };
    return l;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source, layoutVersion, params, width, groups]);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;

  // ── windowing (own, exact: binary search over row tops) ──
  const winY = useRef<[number, number]>([0, -1]);
  const lastTop = useRef(0);
  const update = useCallback(
    (force = false) => {
      const sc = scrollRef.current;
      const l = layoutRef.current;
      if (!sc || !l) return;
      const vh = sc.clientHeight || window.innerHeight;
      const top = sc.scrollTop - marginRef.current;
      const dir = top > lastTop.current + 1 ? 1 : top < lastTop.current - 1 ? -1 : 0;
      lastTop.current = top;
      const vis0 = top;
      const vis1 = top + vh;
      if (l.rowCount) {
        const a = rowAt(l, Math.max(0, vis0));
        const b = rowAt(l, Math.max(0, vis1));
        source.ensureRange(l.rowStart[a], Math.max(l.rowStart[a], l.rowEnd[b] - 1), dir);
      }
      const [y0, y1] = winY.current;
      const pad = vh * 0.35;
      const flying = fastRef.current;
      if (!force && !flying && y1 >= y0 && vis0 >= y0 + (dir < 0 ? pad : 0) && vis1 <= y1 - (dir > 0 ? pad : 0)) return;
      // while dragging the fast scroller only the visible rows are drawn (placeholders, no images)
      const ahead = flying ? vh * 0.1 : vh * 1.15;
      const behind = flying ? vh * 0.1 : vh * 0.35;
      const n0 = Math.max(0, dir < 0 ? vis0 - ahead : vis0 - behind);
      const n1 = dir < 0 ? vis1 + behind : vis1 + ahead;
      winY.current = [n0, n1];
      if (!l.rowCount || vis1 < -vh) {
        setWin((w) => (w[0] === 0 && w[1] === -1 ? w : [0, -1]));
        return;
      }
      const r0 = Math.max(0, rowAt(l, n0));
      const r1 = Math.max(0, rowAt(l, n1));
      setWin((w) => (w[0] === r0 && w[1] === r1 ? w : [r0, r1]));
    },
    [scrollRef, source],
  );

  useLayoutEffect(() => {
    winY.current = [0, -1];
    update(true);
  }, [layout, margin, update]);

  useEffect(() => {
    const sc = scrollRef.current;
    if (!sc) return;
    const on = () => {
      update();
      if (previewRef.current >= 0) setPreview(-1);
    };
    sc.addEventListener('scroll', on, { passive: true });
    const ro = new ResizeObserver(() => update(true));
    ro.observe(sc);
    return () => {
      sc.removeEventListener('scroll', on);
      ro.disconnect();
    };
  }, [scrollRef, update]);
  const previewRef = useRef(preview);
  previewRef.current = preview;

  // ── scrolling helpers ──
  const scrollToIndex = useCallback(
    (i: number, align: 'auto' | 'start' | 'center' = 'auto') => {
      const sc = scrollRef.current;
      const l = layoutRef.current;
      if (!sc || !l || i < 0 || i >= l.total) return;
      const r = l.itemRow[i];
      const y = marginRef.current + l.rowTop[r];
      const h = l.rowH[r];
      const vh = sc.clientHeight;
      if (align === 'auto' && y >= sc.scrollTop + 8 && y + h <= sc.scrollTop + vh - 8) return;
      sc.scrollTop = align === 'start' ? y - 8 : y - (vh - h) / 2;
    },
    [scrollRef],
  );

  useEffect(() => {
    if (!handleRef) return;
    handleRef.current = {
      scrollToIndex,
      scrollToItem(id) {
        const i = source.indexOf(id);
        if (i >= 0) scrollToIndex(i, 'auto');
      },
      scrollToGroup(key) {
        const l = layoutRef.current;
        const g = l.groups.find((x) => x.key === key || x.key.startsWith(key));
        const sc = scrollRef.current;
        if (g && sc) sc.scrollTop = marginRef.current + g.top;
      },
    };
  }, [handleRef, scrollToIndex, source, scrollRef]);

  // ── density (pinch / ctrl+wheel) with an anchored item ──
  const anchor = useRef<{ index: number; offset: number } | null>(null);
  const setLevel = useCallback(
    (next: number, index: number, offset: number) => {
      const n = Math.max(0, Math.min(ZOOM_LEVELS - 1, next));
      const p = usePrefs.getState();
      const cur = phone ? p.gridZoomMobile : p.gridZoomDesktop;
      if (n === cur) return false;
      anchor.current = { index, offset };
      p.set(phone ? { gridZoomMobile: n } : { gridZoomDesktop: n });
      return true;
    },
    [phone],
  );
  useLayoutEffect(() => {
    const a = anchor.current;
    const sc = scrollRef.current;
    if (!a || !sc || a.index < 0 || a.index >= layout.total) return;
    anchor.current = null;
    const r = layout.itemRow[a.index];
    sc.scrollTop = Math.max(0, marginRef.current + layout.rowTop[r] + layout.rowH[r] / 2 - a.offset);
    gridRef.current?.animate?.([{ opacity: 0.35 }, { opacity: 1 }], { duration: 200, easing: 'ease-out' });
    update(true);
  }, [layout, scrollRef, update]);

  const pointToIndex = (clientX: number, clientY: number, nearest = false) => {
    const el = gridRef.current;
    if (!el) return -1;
    const r = el.getBoundingClientRect();
    return (nearest ? nearestItem : itemAt)(layoutRef.current, clientX - r.left, clientY - r.top);
  };

  // ctrl + wheel (desktop / trackpad pinch)
  useEffect(() => {
    const sc = scrollRef.current;
    if (!sc || !zoomable) return;
    let acc = 0;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      acc += e.deltaY;
      if (Math.abs(acc) < 70) return;
      const idx = pointToIndex(e.clientX, e.clientY, true);
      const offset = e.clientY - sc.getBoundingClientRect().top;
      const p = usePrefs.getState();
      setLevel((phone ? p.gridZoomMobile : p.gridZoomDesktop) + (acc > 0 ? 1 : -1), idx, offset);
      acc = 0;
    };
    sc.addEventListener('wheel', onWheel, { passive: false });
    return () => sc.removeEventListener('wheel', onWheel);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scrollRef, zoomable, setLevel, phone]);

  // ── touch: pinch density, long-press + drag range selection ──
  const press = useRef<{ t: ReturnType<typeof setTimeout>; x: number; y: number; index: number } | null>(null);
  const suppressClick = useRef(false);
  const lastPointer = useRef<string>('mouse');
  const touches = useRef(new Set<number>());
  const drag = useRef<RangeDrag | null>(null);
  const pinch = useRef<{ d0: number; cx: number; cy: number; s: number } | null>(null);

  const applyRange = useCallback(() => {
    const d = drag.current;
    if (!d) return;
    const idx = pointToIndex(d.x, d.y, true);
    if (idx < 0 || idx === d.last) return;
    d.last = idx;
    const [a, b] = d.anchor < idx ? [d.anchor, idx] : [idx, d.anchor];
    const n = new Map(d.base);
    for (let i = a; i <= b; i++) {
      const it = source.getAt(i);
      if (!it) continue;
      if (d.adding) n.set(it.id, it);
      else n.delete(it.id);
    }
    useSelection.setState({ items: n, ids: new Set(n.keys()) });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source]);

  const startRange = useCallback(
    (index: number, x: number, y: number) => {
      const it = source.getAt(index);
      if (!it) return;
      const sel = useSelection.getState();
      const adding = !sel.ids.has(it.id);
      const base = new Map(sel.items);
      drag.current = { anchor: index, adding, base, last: -1, x, y, speed: 0, raf: 0 };
      applyRange();
      useSelection.setState({ anchor: index });
      navigator.vibrate?.(12);
      suppressClick.current = true;
    },
    [source, applyRange],
  );

  const autoScroll = useCallback(() => {
    const d = drag.current;
    const sc = scrollRef.current;
    if (!d || !sc) return;
    const r = sc.getBoundingClientRect();
    const edge = 72;
    d.speed = d.y < r.top + edge ? -Math.ceil((r.top + edge - d.y) / 4) : d.y > r.bottom - edge ? Math.ceil((d.y - (r.bottom - edge)) / 4) : 0;
    if (d.speed && !d.raf) {
      const step = () => {
        const dd = drag.current;
        if (!dd || !dd.speed) {
          if (dd) dd.raf = 0;
          return;
        }
        sc.scrollTop += dd.speed;
        dd.last = -1;
        applyRange();
        dd.raf = requestAnimationFrame(step);
      };
      d.raf = requestAnimationFrame(step);
    }
  }, [scrollRef, applyRange]);

  const endDrag = useCallback(() => {
    if (drag.current?.raf) cancelAnimationFrame(drag.current.raf);
    drag.current = null;
  }, []);

  useEffect(() => {
    const el = gridRef.current;
    if (!el) return;
    const dist = (t: TouchList) => Math.hypot(t[0].clientX - t[1].clientX, t[0].clientY - t[1].clientY);
    const onStart = (e: TouchEvent) => {
      if (e.touches.length === 2 && zoomable && !drag.current) {
        if (press.current) clearTimeout(press.current.t);
        press.current = null;
        const r = el.getBoundingClientRect();
        const cx = (e.touches[0].clientX + e.touches[1].clientX) / 2;
        const cy = (e.touches[0].clientY + e.touches[1].clientY) / 2;
        pinch.current = { d0: dist(e.touches), cx, cy, s: 1 };
        el.dataset.pinching = '1';
        el.style.transformOrigin = `${cx - r.left}px ${cy - r.top}px`;
      }
    };
    const onMove = (e: TouchEvent) => {
      const p = pinch.current;
      if (p && e.touches.length === 2) {
        p.s = Math.max(0.5, Math.min(2.2, dist(e.touches) / p.d0));
        el.style.transform = `scale(${p.s})`;
        return;
      }
      const d = drag.current;
      if (d && e.touches.length === 1) {
        d.x = e.touches[0].clientX;
        d.y = e.touches[0].clientY;
        applyRange();
        autoScroll();
      }
    };
    const onEnd = (e: TouchEvent) => {
      const p = pinch.current;
      if (p && e.touches.length < 2) {
        pinch.current = null;
        el.style.transform = '';
        delete el.dataset.pinching;
        const p2 = usePrefs.getState();
        const cur = phone ? p2.gridZoomMobile : p2.gridZoomDesktop;
        const sc = scrollRef.current;
        if (sc && (p.s > 1.15 || p.s < 0.87)) {
          const idx = pointToIndex(p.cx, p.cy, true);
          setLevel(cur + (p.s > 1 ? -1 : 1), idx, p.cy - sc.getBoundingClientRect().top);
        }
        suppressClick.current = true;
        setTimeout(() => (suppressClick.current = false), 400);
      }
      if (!e.touches.length && drag.current) endDrag();
    };
    el.addEventListener('touchstart', onStart, { passive: true });
    el.addEventListener('touchmove', onMove, { passive: true });
    el.addEventListener('touchend', onEnd, { passive: true });
    el.addEventListener('touchcancel', onEnd, { passive: true });
    return () => {
      el.removeEventListener('touchstart', onStart);
      el.removeEventListener('touchmove', onMove);
      el.removeEventListener('touchend', onEnd);
      el.removeEventListener('touchcancel', onEnd);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [zoomable, phone, setLevel, applyRange, autoScroll, endDrag, scrollRef]);

  // While in selection mode the grid gets a NON-passive touchmove listener so a range drag can
  // stop the page from scrolling. (Chrome decides cancelability at touchstart, so it must exist
  // before the gesture; outside selection mode scrolling stays fully async.)
  useEffect(() => {
    const el = gridRef.current;
    if (!el || !selecting) return;
    const block = (e: TouchEvent) => {
      if ((drag.current || pinch.current) && e.cancelable) e.preventDefault();
    };
    el.addEventListener('touchmove', block, { passive: false });
    return () => el.removeEventListener('touchmove', block);
  }, [selecting]);

  // ── pointer: long press (touch), lasso (mouse/pen), hover preview (mouse) ──
  const canHover = useMemo(() => typeof window !== 'undefined' && window.matchMedia('(hover: hover) and (pointer: fine)').matches, []);
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lassoBase = useRef<Map<string, MediaItem>>(new Map());

  const tileIndex = (target: EventTarget | null): number => {
    const el = (target as HTMLElement | null)?.closest?.('[data-index]');
    return el ? Number(el.getAttribute('data-index')) : -1;
  };

  const onPointerDown = (e: React.PointerEvent) => {
    lastPointer.current = e.pointerType;
    if (e.button !== 0) return;
    const idx = tileIndex(e.target);
    if (e.pointerType === 'touch') {
      touches.current.add(e.pointerId);
      // a second finger (pinch) cancels the long press
      if (touches.current.size > 1 || pinch.current) {
        if (press.current) clearTimeout(press.current.t);
        press.current = null;
        return;
      }
      if (idx < 0 || !source.getAt(idx)) return;
      // range drag straight from a visible check circle
      if (selecting && (e.target as HTMLElement).closest('[data-check]')) {
        startRange(idx, e.clientX, e.clientY);
        return;
      }
      suppressClick.current = false;
      const x = e.clientX;
      const y = e.clientY;
      press.current = {
        x,
        y,
        index: idx,
        t: setTimeout(() => {
          press.current = null;
          if (touches.current.size > 1 || pinch.current) return;
          startRange(idx, x, y);
        }, 430),
      };
      return;
    }
    // mouse / pen: lasso on empty space
    if (idx >= 0 || (e.target as HTMLElement).closest('button')) return;
    const r = gridRef.current!.getBoundingClientRect();
    const x = e.clientX - r.left;
    const y = e.clientY - r.top;
    lassoBase.current = e.ctrlKey || e.metaKey || e.shiftKey ? new Map(useSelection.getState().items) : new Map();
    if (!lassoBase.current.size) useSelection.getState().clear();
    setLasso({ x0: x, y0: y, x1: x, y1: y });
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const p = press.current;
    if (p && Math.hypot(e.clientX - p.x, e.clientY - p.y) > 10) {
      clearTimeout(p.t);
      press.current = null;
    }
    if (!lasso) return;
    const l = layoutRef.current;
    const r = gridRef.current!.getBoundingClientRect();
    const next = { ...lasso, x1: e.clientX - r.left, y1: e.clientY - r.top };
    setLasso(next);
    const minX = Math.min(next.x0, next.x1);
    const maxX = Math.max(next.x0, next.x1);
    const minY = Math.min(next.y0, next.y1);
    const maxY = Math.max(next.y0, next.y1);
    if (maxX - minX < 4 && maxY - minY < 4) return;
    const hit = new Map(lassoBase.current);
    for (let row = Math.max(0, rowAt(l, minY)); row < l.rowCount && l.rowTop[row] < maxY; row++) {
      if (l.rowHeader[row] || l.rowTop[row] + l.rowH[row] < minY) continue;
      for (let i = l.rowStart[row]; i < l.rowEnd[row]; i++) {
        if (l.itemX[i] < maxX && l.itemX[i] + l.itemW[i] > minX) {
          const it = source.getAt(i);
          if (it) hit.set(it.id, it);
        }
      }
    }
    useSelection.setState({ items: hit, ids: new Set(hit.keys()) });
    const sc = scrollRef.current;
    if (sc) {
      const sr = sc.getBoundingClientRect();
      if (e.clientY > sr.bottom - 40) sc.scrollBy(0, 18);
      else if (e.clientY < sr.top + 40) sc.scrollBy(0, -18);
    }
  };

  const onPointerUp = (e: React.PointerEvent) => {
    touches.current.delete(e.pointerId);
    if (press.current) clearTimeout(press.current.t);
    press.current = null;
    if (lasso) setLasso(null);
  };

  const rangeSelect = useCallback(
    async (a: number, b: number) => {
      const [s, e] = a < b ? [a, b] : [b, a];
      const list = await source.itemsInRange(s, e);
      useSelection.getState().add(list);
    },
    [source],
  );

  const onClick = (e: React.MouseEvent) => {
    if (suppressClick.current) {
      suppressClick.current = false;
      return;
    }
    const target = e.target as HTMLElement;
    const g = target.closest('[data-group-select]');
    if (g) {
      const grp = layoutRef.current.groups[Number(g.getAttribute('data-group-select'))];
      if (grp) {
        void source.itemsInRange(grp.start, grp.start + grp.count - 1).then((list) => {
          const sel = useSelection.getState();
          if (list.length && list.every((i) => sel.ids.has(i.id))) sel.remove(list.map((i) => i.id));
          else sel.add(list);
        });
      }
      return;
    }
    const idx = tileIndex(target);
    if (idx < 0) return;
    const item = source.getAt(idx);
    if (!item) return;
    const sel = useSelection.getState();
    const onCheck = !!target.closest('[data-check]');
    if (e.shiftKey && sel.anchor != null) {
      void rangeSelect(sel.anchor, idx);
      useSelection.setState({ anchor: idx });
      return;
    }
    if (onCheck || e.ctrlKey || e.metaKey || sel.ids.size > 0) {
      sel.toggle(item, idx);
      return;
    }
    onOpen(item, idx);
  };

  const onContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    // Android fires contextmenu on long press — that gesture means "select" here
    if (lastPointer.current === 'touch') return;
    const idx = tileIndex(e.target);
    const item = idx >= 0 ? source.getAt(idx) : undefined;
    if (!item) return;
    const sel = useSelection.getState();
    const list = sel.ids.has(item.id) ? sel.selected() : [item];
    setMenu({ at: { x: e.clientX, y: e.clientY }, items: list, index: idx });
  };

  const onPointerOver = (e: React.PointerEvent) => {
    if (e.pointerType !== 'mouse' || !canHover || !hoverPreviewPref) return;
    const idx = tileIndex(e.target);
    if (idx === previewRef.current) return;
    if (hoverTimer.current) clearTimeout(hoverTimer.current);
    if (previewRef.current >= 0) setPreview(-1);
    const it = idx >= 0 ? source.getAt(idx) : undefined;
    if (it?.kind === 'video') hoverTimer.current = setTimeout(() => setPreview(idx), 450);
  };
  const onPointerLeave = () => {
    if (hoverTimer.current) clearTimeout(hoverTimer.current);
    if (previewRef.current >= 0) setPreview(-1);
  };

  // ── keyboard navigation ──
  const focusIndex = (i: number) => {
    scrollToIndex(i, 'auto');
    const el = gridRef.current?.querySelector<HTMLElement>(`[data-index="${i}"]`);
    if (el) el.focus({ preventScroll: true });
    else requestAnimationFrame(() => gridRef.current?.querySelector<HTMLElement>(`[data-index="${i}"]`)?.focus({ preventScroll: true }));
  };
  const onKeyDown = (e: React.KeyboardEvent) => {
    const l = layoutRef.current;
    if (!l.total) return;
    let idx = tileIndex(document.activeElement);
    if (idx < 0) {
      if (!['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight'].includes(e.key)) return;
      const sc = scrollRef.current!;
      idx = nearestItem(l, 0, Math.max(0, sc.scrollTop - marginRef.current + 10));
      e.preventDefault();
      focusIndex(Math.max(0, idx));
      return;
    }
    const item = source.getAt(idx);
    const vertical = (d: 1 | -1) => {
      let r = l.itemRow[idx] + d;
      while (r >= 0 && r < l.rowCount && l.rowHeader[r]) r += d;
      if (r < 0 || r >= l.rowCount) return idx;
      const cx = l.itemX[idx] + l.itemW[idx] / 2;
      let best = l.rowStart[r];
      for (let i = l.rowStart[r]; i < l.rowEnd[r]; i++) if (l.itemX[i] <= cx) best = i;
      return best;
    };
    let next = idx;
    if (e.key === 'ArrowRight') next = Math.min(l.total - 1, idx + 1);
    else if (e.key === 'ArrowLeft') next = Math.max(0, idx - 1);
    else if (e.key === 'ArrowDown') next = vertical(1);
    else if (e.key === 'ArrowUp') next = vertical(-1);
    else if (e.key === 'Enter' && item) {
      e.preventDefault();
      onOpen(item, idx);
      return;
    } else if (e.key === ' ' && item) {
      e.preventDefault();
      useSelection.getState().toggle(item, idx);
      return;
    } else return;
    e.preventDefault();
    if (e.shiftKey) {
      const it = source.getAt(next);
      if (it) useSelection.getState().add([it]);
    }
    focusIndex(next);
  };

  // ── fast scroller ──
  const dateSort = !sortKey || sortKey === 'takenAt';
  const labelAt = useCallback(
    (gy: number) => {
      const l = layoutRef.current;
      if (!l.rowCount) return '';
      const r = Math.max(0, rowAt(l, Math.max(0, gy)));
      if (l.groupBy) {
        const g = l.groups[l.rowGroup[r]];
        return g?.key ? formatMonthKey(g.key, lang) : '';
      }
      const it = source.getAt(l.rowStart[r]);
      if (!it) return '…';
      switch (sortKey) {
        case 'name':
          return it.name.charAt(0).toLocaleUpperCase(lang);
        case 'size':
          return formatBytes(it.size);
        case 'mtime':
          return formatLocal(it.mtime - new Date(it.mtime).getTimezoneOffset() * 60000, lang, { month: 'long', year: 'numeric' });
        case 'kind':
          return t(it.kind === 'video' ? 'gallery.filter.video' : 'gallery.filter.image');
        default:
          return formatLocal(it.localAt, lang, { month: 'long', year: 'numeric' });
      }
    },
    [source, lang, sortKey, t],
  );
  const ticks: FastScrollerTick[] = useMemo(() => {
    if (!layout.groupBy || !dateSort) return [];
    const out: FastScrollerTick[] = [];
    let year = '';
    for (const g of layout.groups) {
      const y = g.key.slice(0, 4);
      if (y !== year) {
        year = y;
        out.push({ label: y, y: g.top });
      }
    }
    return out;
  }, [layout, dateSort]);

  const onFastDrag = useCallback(
    (d: boolean) => {
      fastRef.current = d;
      setFast(d);
      if (!d) requestAnimationFrame(() => update(true));
    },
    [update],
  );

  // ── render window ──
  const dpr = typeof window !== 'undefined' ? Math.min(2.5, window.devicePixelRatio || 1) : 1;
  const nodes: React.ReactNode[] = [];
  const [r0, r1] = win;
  for (let r = r0; r <= r1 && r < layout.rowCount; r++) {
    const y = layout.rowTop[r];
    const h = layout.rowH[r];
    if (layout.rowHeader[r]) {
      const gi = layout.rowGroup[r];
      const g = layout.groups[gi];
      if (!g) continue;
      const label = layout.groupBy === 'month' ? formatMonthKey(g.key, lang) : formatDayKey(g.key, lang);
      nodes.push(<GroupHeader key={`h${g.key}`} y={y} h={h} label={label} place={g.place} count={g.count} index={gi} phone={phone} />);
      continue;
    }
    const size = h * dpr > 330 ? 640 : 320;
    for (let i = layout.rowStart[r]; i < layout.rowEnd[r]; i++) {
      nodes.push(
        <MediaTile key={i} index={i} item={source.getAt(i)} x={layout.itemX[i]} y={y} w={layout.itemW[i]} h={h} size={size} fast={fast} preview={preview === i} selecting={selecting} />,
      );
    }
  }

  return (
    <>
      <div
        ref={gridRef}
        className="grid-root"
        data-grid
        data-phone={phone ? '' : undefined}
        data-selecting={selecting ? '' : undefined}
        style={{ height: Math.max(layout.height, 1) }}
        tabIndex={0}
        role="grid"
        aria-rowcount={layout.rowCount}
        aria-label={t('gallery.media')}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onPointerOver={onPointerOver}
        onPointerLeave={onPointerLeave}
        onClick={onClick}
        onContextMenu={onContextMenu}
        onKeyDown={onKeyDown}
      >
        {nodes}
        {lasso && (
          <div
            className="pointer-events-none absolute z-10 rounded-md border border-[var(--accent)] bg-[color-mix(in_oklab,var(--accent)_14%,transparent)]"
            style={{ left: Math.min(lasso.x0, lasso.x1), top: Math.min(lasso.y0, lasso.y1), width: Math.abs(lasso.x1 - lasso.x0), height: Math.abs(lasso.y1 - lasso.y0) }}
          />
        )}
      </div>
      {fastScroller && layout.height > 0 && <FastScroller scrollRef={scrollRef} marginRef={marginRef} labelAt={labelAt} ticks={ticks} onDrag={onFastDrag} bottomInset={phone ? 64 : 0} />}
      <ContextMenu at={menu?.at ?? null} onClose={() => setMenu(null)}>
        {menu && <MediaMenu items={menu.items} onOpen={menu.items.length === 1 ? () => onOpen(menu.items[0], menu.index) : undefined} />}
      </ContextMenu>
    </>
  );
}
