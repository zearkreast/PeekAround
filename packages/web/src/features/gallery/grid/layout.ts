/**
 * Exact grid layout engine.
 *
 * Lays out EVERY item of a browse result up front from the skeleton aspect bytes
 * (see BrowseSkeleton) — no item data needed — so the total height, every row and
 * every tile position are known before a single page is loaded. Typed arrays keep
 * 100k items in a few milliseconds and a few MB.
 */
import { byteToAspect } from '@peekaround/shared';

export type GroupBy = 'day' | 'month' | null;

export interface SkeletonDay {
  key: string; // YYYY-MM-DD (localAt)
  count: number;
  place: string | null;
}

export interface LayoutGroup {
  /** 'YYYY-MM-DD' (day) or 'YYYY-MM' (month) */
  key: string;
  place: string | null;
  start: number;
  count: number;
  /** row index of the header */
  row: number;
  top: number;
}

export interface GridLayout {
  total: number;
  width: number;
  height: number;
  rowCount: number;
  rowTop: Float64Array;
  rowH: Float32Array;
  /** first item index of a row (header rows: first item of their group) */
  rowStart: Int32Array;
  /** exclusive end (header rows: === rowStart) */
  rowEnd: Int32Array;
  /** 1 = header row */
  rowHeader: Uint8Array;
  rowGroup: Int32Array;
  itemX: Float32Array;
  itemW: Float32Array;
  itemRow: Int32Array;
  groups: LayoutGroup[];
  groupBy: GroupBy;
}

export interface LayoutOptions {
  width: number;
  /** target row height (justified) or tile size (square) */
  target: number;
  gap: number;
  square: boolean;
  groupBy: GroupBy;
  headerH: number;
}

const MIN_A = 0.42;
const MAX_A = 3.2;
const ASPECT_LUT = new Float32Array(256);
for (let b = 0; b < 256; b++) ASPECT_LUT[b] = Math.max(MIN_A, Math.min(MAX_A, byteToAspect(b)));

/** Merge consecutive day groups into month groups. */
export function monthGroups(days: SkeletonDay[]): SkeletonDay[] {
  const out: SkeletonDay[] = [];
  for (const d of days) {
    const k = d.key.slice(0, 7);
    const last = out[out.length - 1];
    if (last && last.key === k) {
      last.count += d.count;
      if (!last.place) last.place = d.place;
    } else out.push({ key: k, count: d.count, place: d.place });
  }
  return out;
}

export function layoutGrid(aspects: Uint8Array, total: number, days: SkeletonDay[] | null, o: LayoutOptions): GridLayout {
  const width = Math.max(0, o.width);
  const gap = o.gap;
  const groups: SkeletonDay[] =
    o.groupBy && days && days.length ? (o.groupBy === 'month' ? monthGroups(days) : days) : [{ key: '', count: total, place: null }];
  const grouped = !!(o.groupBy && days && days.length);

  // worst case rows: one per item + one header per group
  let cap = Math.max(16, total + groups.length + 1);
  let rowTop = new Float64Array(cap);
  let rowH = new Float32Array(cap);
  let rowStart = new Int32Array(cap);
  let rowEnd = new Int32Array(cap);
  let rowHeader = new Uint8Array(cap);
  let rowGroup = new Int32Array(cap);
  const itemX = new Float32Array(total);
  const itemW = new Float32Array(total);
  const itemRow = new Int32Array(total);
  const outGroups: LayoutGroup[] = [];

  let rows = 0;
  let y = 0;
  const pushRow = (top: number, h: number, s: number, e: number, header: number, g: number) => {
    if (rows >= cap) {
      cap *= 2;
      const grow = <T extends Float64Array | Float32Array | Int32Array | Uint8Array>(a: T, C: { new (n: number): T }) => {
        const n = new C(cap);
        n.set(a);
        return n;
      };
      rowTop = grow(rowTop, Float64Array);
      rowH = grow(rowH, Float32Array);
      rowStart = grow(rowStart, Int32Array);
      rowEnd = grow(rowEnd, Int32Array);
      rowHeader = grow(rowHeader, Uint8Array);
      rowGroup = grow(rowGroup, Int32Array);
    }
    rowTop[rows] = top;
    rowH[rows] = h;
    rowStart[rows] = s;
    rowEnd[rows] = e;
    rowHeader[rows] = header;
    rowGroup[rows] = g;
    return rows++;
  };

  const finishRow = (s: number, e: number, h: number, stretch: boolean, g: number) => {
    const hh = Math.max(1, Math.round(h));
    const r = pushRow(y, hh, s, e, 0, g);
    const n = e - s;
    const available = width - gap * (n - 1);
    let x = 0;
    let used = 0;
    for (let i = s; i < e; i++) {
      let w = ASPECT_LUT[aspects[i] ?? 128] * h;
      if (stretch) w = i === e - 1 ? available - used : Math.round(w);
      else w = Math.round(w);
      used += w;
      itemX[i] = x;
      itemW[i] = w;
      itemRow[i] = r;
      x += w + gap;
    }
    y += hh + gap;
  };

  if (width > 0 && total > 0) {
    let start = 0;
    const cols = Math.max(2, Math.round((width + gap) / (o.target + gap)));
    const size = (width - gap * (cols - 1)) / cols;
    for (let g = 0; g < groups.length; g++) {
      const count = Math.min(groups[g].count, total - start);
      if (count <= 0) break;
      const end = start + count;
      if (grouped) {
        const r = pushRow(y, o.headerH, start, start, 1, g);
        outGroups.push({ key: groups[g].key, place: groups[g].place, start, count, row: r, top: y });
        y += o.headerH;
      } else outGroups.push({ key: '', place: null, start, count, row: rows, top: y });
      if (o.square) {
        const sz = Math.round(size);
        for (let s = start; s < end; s += cols) {
          const e = Math.min(end, s + cols);
          const r = pushRow(y, sz, s, e, 0, g);
          for (let i = s; i < e; i++) {
            const k = i - s;
            const x0 = Math.round(k * (size + gap));
            const x1 = k === cols - 1 ? width : Math.round(k * (size + gap) + size);
            itemX[i] = x0;
            itemW[i] = x1 - x0;
            itemRow[i] = r;
          }
          y += sz + gap;
        }
      } else {
        let rs = start;
        let sum = 0;
        for (let i = start; i < end; i++) {
          sum += ASPECT_LUT[aspects[i] ?? 128];
          const n = i - rs + 1;
          if (sum * o.target + gap * (n - 1) >= width) {
            finishRow(rs, i + 1, (width - gap * (n - 1)) / sum, true, g);
            rs = i + 1;
            sum = 0;
          }
        }
        if (rs < end) {
          const n = end - rs;
          finishRow(rs, end, Math.min(o.target, (width - gap * (n - 1)) / sum), false, g);
        }
      }
      start = end;
    }
    // items the skeleton groups did not cover (defensive): one more ungrouped block
    if (start < total) {
      let rs = start;
      let sum = 0;
      for (let i = start; i < total; i++) {
        sum += ASPECT_LUT[aspects[i] ?? 128];
        const n = i - rs + 1;
        if (sum * o.target + gap * (n - 1) >= width) {
          finishRow(rs, i + 1, (width - gap * (n - 1)) / sum, true, groups.length - 1);
          rs = i + 1;
          sum = 0;
        }
      }
      if (rs < total) finishRow(rs, total, Math.min(o.target, (width - gap * (total - rs - 1)) / sum), false, groups.length - 1);
    }
  }
  const height = rows ? rowTop[rows - 1] + rowH[rows - 1] : 0;
  return {
    total,
    width,
    height,
    rowCount: rows,
    rowTop,
    rowH,
    rowStart,
    rowEnd,
    rowHeader,
    rowGroup,
    itemX,
    itemW,
    itemRow,
    groups: outGroups,
    groupBy: grouped ? o.groupBy : null,
  };
}

/** Index of the last row whose top is <= y (0 when y is above the first row). */
export function rowAt(l: GridLayout, y: number): number {
  let lo = 0;
  let hi = l.rowCount - 1;
  if (hi < 0) return -1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (l.rowTop[mid] <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** Item under a point (layout coordinates) or -1. */
export function itemAt(l: GridLayout, x: number, y: number): number {
  const r = rowAt(l, y);
  if (r < 0 || l.rowHeader[r] || y < l.rowTop[r] || y > l.rowTop[r] + l.rowH[r]) return -1;
  for (let i = l.rowStart[r]; i < l.rowEnd[r]; i++) if (x >= l.itemX[i] && x < l.itemX[i] + l.itemW[i]) return i;
  return -1;
}

/** Nearest item to a point, also when the point is in a gap/header (range selection). */
export function nearestItem(l: GridLayout, x: number, y: number): number {
  if (!l.total) return -1;
  let r = rowAt(l, y);
  if (r < 0) return 0;
  while (r < l.rowCount && l.rowHeader[r]) r++;
  if (r >= l.rowCount) return l.total - 1;
  const s = l.rowStart[r];
  const e = l.rowEnd[r];
  for (let i = s; i < e; i++) if (x < l.itemX[i] + l.itemW[i]) return i;
  return e - 1;
}

export function itemTop(l: GridLayout, i: number): number {
  return l.rowTop[l.itemRow[i]] ?? 0;
}

export function groupOfRow(l: GridLayout, r: number): LayoutGroup | undefined {
  return l.groups[l.rowGroup[r]] ?? l.groups[0];
}
