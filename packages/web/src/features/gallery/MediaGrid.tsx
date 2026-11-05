import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent, type RefObject } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { CheckCircle2 } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useLang } from '@/i18n';
import { formatDate, formatMonth } from '@/lib/format';
import { aspectOf, justify, squares, type JRow } from '@/lib/justified';
import { usePrefs } from '@/store/prefs';
import { useSelection } from '@/store/selection';
import { ContextMenu } from '@/components/ui';
import { MediaMenu } from './actions';
import { MediaTile } from './MediaTile';

type VRow = { kind: 'row'; row: JRow; height: number } | { kind: 'header'; label: string; count: number; height: number; key: string };

const GAP = 6;
const gapFor = (w: number) => (w < 700 ? 3 : GAP);
const HEADER_H = 58;

function groupKey(ms: number, by: 'month' | 'day') {
  const d = new Date(ms);
  const m = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
  return by === 'month' ? m : `${m}-${String(d.getDate()).padStart(2, '0')}`;
}

export interface MediaGridHandle {
  scrollToItem(id: string): void;
  scrollToGroup(key: string): void;
}

export function MediaGrid({
  items,
  hasMore,
  loadMore,
  scrollRef,
  groupBy = null,
  onOpen,
  handleRef,
}: {
  items: MediaItem[];
  hasMore: boolean;
  loadMore: () => void;
  scrollRef: RefObject<HTMLElement | null>;
  groupBy?: 'month' | 'day' | null;
  onOpen: (item: MediaItem) => void;
  handleRef?: RefObject<MediaGridHandle | null>;
}) {
  const lang = useLang();
  const { rowHeight, gridMode } = usePrefs();
  const selection = useSelection();
  const gridRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [margin, setMargin] = useState(0);
  const [menu, setMenu] = useState<{ at: { x: number; y: number }; items: MediaItem[] } | null>(null);
  const [lasso, setLasso] = useState<{ x0: number; y0: number; x1: number; y1: number } | null>(null);

  useLayoutEffect(() => {
    const el = gridRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  // offset of the grid inside the scroll container (content above it changes size)
  useLayoutEffect(() => {
    const sc = scrollRef.current;
    const el = gridRef.current;
    if (!sc || !el) return;
    const measure = () => {
      const m = el.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
      setMargin((prev) => (Math.abs(prev - m) > 0.5 ? m : prev));
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (sc.firstElementChild) ro.observe(sc.firstElementChild);
    ro.observe(sc);
    return () => ro.disconnect();
  }, [scrollRef]);

  const { rows, positions } = useMemo(() => {
    const out: VRow[] = [];
    // phones: keep ~3 tiles per row instead of one huge one
    const target = width < 700 ? Math.min(rowHeight, Math.max(96, width / 3.4)) : rowHeight;
    const gap = gapFor(width);
    const layout = (list: MediaItem[], start: number) =>
      gridMode === 'square' ? squares(list.length, width, target * 0.85, gap, start) : justify(list.map((i) => aspectOf(i.width, i.height)), width, target, gap, start);
    if (width > 0) {
      if (groupBy) {
        let s = 0;
        while (s < items.length) {
          const key = groupKey(items[s].takenAt, groupBy);
          let e = s + 1;
          while (e < items.length && groupKey(items[e].takenAt, groupBy) === key) e++;
          const label = groupBy === 'month' ? formatMonth(key, lang) : formatDate(items[s].takenAt, lang, { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
          out.push({ kind: 'header', label, count: e - s, height: HEADER_H, key });
          for (const r of layout(items.slice(s, e), s)) out.push({ kind: 'row', row: r, height: r.height });
          s = e;
        }
      } else {
        for (const r of layout(items, 0)) out.push({ kind: 'row', row: r, height: r.height });
      }
    }
    // absolute positions for lasso hit testing
    const pos: { id: string; x: number; y: number; w: number; h: number }[] = [];
    let y = 0;
    for (const r of out) {
      if (r.kind === 'row') r.row.widths.forEach((w, k) => pos.push({ id: items[r.row.start + k].id, x: r.row.xs[k], y, w, h: r.row.height }));
      y += r.height + gap;
    }
    return { rows: out, positions: pos };
  }, [items, width, rowHeight, gridMode, groupBy, lang]);

  const virt = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => rows[i].height + gapFor(width),
    overscan: 4,
    scrollMargin: margin,
  });

  useEffect(() => {
    virt.measure();
  }, [rows, virt]);

  const vItems = virt.getVirtualItems();
  const last = vItems[vItems.length - 1];
  useEffect(() => {
    if (hasMore && last && last.index >= rows.length - 6) loadMore();
  }, [last?.index, rows.length, hasMore, loadMore, last]);

  useEffect(() => {
    if (!handleRef) return;
    handleRef.current = {
      scrollToItem(id) {
        const idx = items.findIndex((i) => i.id === id);
        if (idx < 0) return;
        const r = rows.findIndex((x) => x.kind === 'row' && idx >= x.row.start && idx < x.row.end);
        if (r >= 0) virt.scrollToIndex(r, { align: 'auto' });
      },
      scrollToGroup(key) {
        const r = rows.findIndex((x) => x.kind === 'header' && x.key === key);
        if (r >= 0) virt.scrollToIndex(r, { align: 'start', behavior: 'smooth' });
      },
    };
  }, [handleRef, items, rows, virt]);

  const selecting = selection.ids.size > 0;

  // entrance animation only right after the first items arrive (not while scrolling)
  const [intro, setIntro] = useState(true);
  const hasItems = items.length > 0;
  useEffect(() => {
    if (!hasItems) return;
    const tm = setTimeout(() => setIntro(false), 1400);
    return () => clearTimeout(tm);
  }, [hasItems]);

  const onClick = useCallback(
    (item: MediaItem, e: MouseEvent) => {
      const sel = useSelection.getState();
      if (e.shiftKey && sel.anchor) {
        const a = items.findIndex((i) => i.id === sel.anchor);
        const b = items.findIndex((i) => i.id === item.id);
        if (a >= 0 && b >= 0) {
          const [s, t] = a < b ? [a, b] : [b, a];
          sel.add(items.slice(s, t + 1).map((i) => i.id));
          return;
        }
      }
      if (e.ctrlKey || e.metaKey || sel.ids.size > 0) {
        sel.toggle(item.id);
        return;
      }
      onOpen(item);
    },
    [items, onOpen],
  );

  const onToggle = useCallback(
    (item: MediaItem, e: MouseEvent) => {
      const sel = useSelection.getState();
      if (e.shiftKey && sel.anchor) {
        const a = items.findIndex((i) => i.id === sel.anchor);
        const b = items.findIndex((i) => i.id === item.id);
        if (a >= 0 && b >= 0) {
          const [s, t] = a < b ? [a, b] : [b, a];
          sel.add(items.slice(s, t + 1).map((i) => i.id));
          return;
        }
      }
      sel.toggle(item.id);
    },
    [items],
  );

  const onContext = useCallback(
    (item: MediaItem, e: MouseEvent) => {
      const sel = useSelection.getState();
      const list = sel.ids.has(item.id) ? items.filter((i) => sel.ids.has(i.id)) : [item];
      setMenu({ at: { x: e.clientX, y: e.clientY }, items: list });
    },
    [items],
  );

  // ── lasso selection ──
  const lassoBase = useRef<Set<string>>(new Set());
  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest('[data-tile]')) return;
    const r = gridRef.current!.getBoundingClientRect();
    const x = e.clientX - r.left;
    const y = e.clientY - r.top;
    lassoBase.current = e.ctrlKey || e.metaKey || e.shiftKey ? new Set(useSelection.getState().ids) : new Set();
    if (!lassoBase.current.size) useSelection.getState().clear();
    setLasso({ x0: x, y0: y, x1: x, y1: y });
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    if (!lasso) return;
    const r = gridRef.current!.getBoundingClientRect();
    const next = { ...lasso, x1: e.clientX - r.left, y1: e.clientY - r.top };
    setLasso(next);
    const minX = Math.min(next.x0, next.x1);
    const maxX = Math.max(next.x0, next.x1);
    const minY = Math.min(next.y0, next.y1);
    const maxY = Math.max(next.y0, next.y1);
    if (maxX - minX < 4 && maxY - minY < 4) return;
    const hit = new Set(lassoBase.current);
    for (const p of positions) if (p.x < maxX && p.x + p.w > minX && p.y < maxY && p.y + p.h > minY) hit.add(p.id);
    useSelection.getState().set(hit, useSelection.getState().anchor);
    // auto scroll near edges
    const sc = scrollRef.current;
    if (sc) {
      const sr = sc.getBoundingClientRect();
      if (e.clientY > sr.bottom - 40) sc.scrollBy(0, 18);
      else if (e.clientY < sr.top + 40) sc.scrollBy(0, -18);
    }
  };
  const onPointerUp = () => setLasso(null);

  let tileIndex = 0;
  return (
    <>
      <div
        ref={gridRef}
        className="relative w-full"
        style={{ height: Math.max(virt.getTotalSize(), 1) }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      >
        {vItems.map((v) => {
          const r = rows[v.index];
          const top = v.start - margin;
          if (r.kind === 'header') {
            return (
              <div key={v.key} className="absolute inset-x-0 flex items-end gap-3 pb-3" style={{ top, height: r.height }}>
                <h3 className="font-display text-lg font-semibold capitalize">{r.label}</h3>
                <span className="pb-0.5 text-xs text-faint">{r.count}</span>
                <button
                  type="button"
                  title="Select"
                  className="ml-auto grid size-7 place-items-center rounded-full text-faint transition-colors hover:bg-surface-2 hover:text-accent"
                  onClick={() => {
                    const ids = items.filter((i) => groupKey(i.takenAt, groupBy!) === r.key).map((i) => i.id);
                    useSelection.getState().add(ids);
                  }}
                >
                  <CheckCircle2 className="size-4" />
                </button>
              </div>
            );
          }
          return (
            <div key={v.key} className="absolute inset-x-0" style={{ top, height: r.height }}>
              {r.row.widths.map((w, k) => {
                const item = items[r.row.start + k];
                if (!item) return null;
                return (
                  <MediaTile
                    key={item.id}
                    item={item}
                    style={{ left: r.row.xs[k], width: w, height: r.row.height }}
                    selected={selection.ids.has(item.id)}
                    selecting={selecting}
                    delay={intro ? tileIndex++ : -1}
                    onClick={onClick}
                    onToggle={onToggle}
                    onContext={onContext}
                  />
                );
              })}
            </div>
          );
        })}
        {lasso && (
          <div
            className="pointer-events-none absolute z-10 rounded-md border border-[var(--accent)] bg-[color-mix(in_oklab,var(--accent)_14%,transparent)]"
            style={{ left: Math.min(lasso.x0, lasso.x1), top: Math.min(lasso.y0, lasso.y1), width: Math.abs(lasso.x1 - lasso.x0), height: Math.abs(lasso.y1 - lasso.y0) }}
          />
        )}
      </div>
      <ContextMenu at={menu?.at ?? null} onClose={() => setMenu(null)}>
        {menu && <MediaMenu items={menu.items} onOpen={menu.items.length === 1 ? () => onOpen(menu.items[0]) : undefined} />}
      </ContextMenu>
    </>
  );
}
