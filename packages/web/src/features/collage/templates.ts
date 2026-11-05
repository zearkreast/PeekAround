import type { MessageKey } from '@/i18n';

/** A cell in normalized content coordinates (0…1). Free templates use x/y as the centre. */
export interface CellSpec {
  x: number;
  y: number;
  w: number;
  h: number;
  rot?: number;
  style?: 'plain' | 'polaroid' | 'print';
}

export interface Template {
  id: string;
  label: MessageKey;
  /** "grid" cells tile the content box (gap applies); "free" cells float, rotate and overlap */
  kind: 'grid' | 'free';
  make(n: number, aspect: number, seed: number): CellSpec[];
}

function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Lay `counts[i]` cells in row i; row heights / cell widths follow the weights. */
function rowsLayout(counts: number[], rowWeights?: number[], cellWeights?: (row: number, n: number) => number[]): CellSpec[] {
  const rw = rowWeights ?? counts.map(() => 1);
  const total = rw.reduce((a, b) => a + b, 0);
  const out: CellSpec[] = [];
  let y = 0;
  counts.forEach((n, r) => {
    const h = rw[r] / total;
    const cw = cellWeights?.(r, n) ?? Array.from({ length: n }, () => 1);
    const ct = cw.reduce((a, b) => a + b, 0);
    let x = 0;
    for (let i = 0; i < n; i++) {
      const w = cw[i] / ct;
      out.push({ x, y, w, h });
      x += w;
    }
    y += h;
  });
  return out;
}

const transpose = (cells: CellSpec[]) => cells.map((c) => ({ ...c, x: c.y, y: c.x, w: c.h, h: c.w }));

function even(n: number, groups: number) {
  const base = Math.floor(n / groups);
  const extra = n % groups;
  return Array.from({ length: groups }, (_, i) => base + (i < extra ? 1 : 0));
}

/** Columns that make cells closest to square for this canvas aspect. */
function bestCols(n: number, aspect: number) {
  let best = 1;
  let score = Infinity;
  for (let c = 1; c <= n; c++) {
    const r = Math.ceil(n / c);
    const cell = (aspect * r) / c;
    const s = Math.abs(Math.log(cell)) + (c * r - n) * 0.25;
    if (s < score) (score = s), (best = c);
  }
  return best;
}

function gridCells(n: number, aspect: number) {
  const c = bestCols(n, aspect);
  const r = Math.ceil(n / c);
  const counts = Array.from({ length: r }, (_, i) => (i < r - 1 ? c : n - c * (r - 1)));
  return rowsLayout(counts);
}

function scatter(n: number, aspect: number, seed: number, style: 'polaroid' | 'print'): CellSpec[] {
  const rand = rng(seed * 97 + n);
  const c = bestCols(n, aspect);
  const r = Math.ceil(n / c);
  // frame aspect in pixels → normalized
  const out: CellSpec[] = [];
  for (let i = 0; i < n; i++) {
    const row = Math.floor(i / c);
    const inRow = row === r - 1 ? n - c * (r - 1) : c;
    const col = i % c;
    const pixAspect = style === 'polaroid' ? 0.84 : i % 3 === 1 ? 0.78 : 1.3;
    const k = style === 'polaroid' ? 1.02 : 1.14;
    let w = k / c;
    let h = (w * aspect) / pixAspect;
    if (h > k / r) (h = k / r), (w = (h * pixAspect) / aspect);
    const jx = (rand() - 0.5) * (0.22 / c);
    const jy = (rand() - 0.5) * (0.22 / r);
    const offset = (c - inRow) / 2;
    // keep each photo inside the canvas (rotation may still nudge a corner out)
    const keep = (v: number, half: number) => (half >= 0.5 ? 0.5 : Math.min(1 - half, Math.max(half, v)));
    out.push({
      x: keep((col + offset + 0.5) / c + jx, w / 2),
      y: keep((row + 0.5) / r + jy, h / 2),
      w,
      h,
      rot: (rand() - 0.5) * (style === 'polaroid' ? 16 : 22),
      style,
    });
  }
  return out;
}

export const TEMPLATES: Template[] = [
  { id: 'grid', label: 'collage.tpl.grid', kind: 'grid', make: (n, a) => gridCells(n, a) },
  {
    id: 'mosaic',
    label: 'collage.tpl.mosaic',
    kind: 'grid',
    make: (n) => {
      const pattern: Record<number, number[]> = { 2: [2], 3: [1, 2], 4: [2, 2], 5: [2, 3], 6: [3, 3], 7: [2, 3, 2], 8: [3, 2, 3], 9: [3, 3, 3] };
      const counts = pattern[n] ?? [n];
      return rowsLayout(
        counts,
        counts.map((_, i) => (i % 2 ? 1 : 1.25)),
        (row, k) => (k === 1 ? [1] : k === 2 ? (row % 2 ? [2, 3] : [3, 2]) : row % 2 ? [1, 1.5, 1] : [1.4, 1, 1.2]),
      );
    },
  },
  {
    id: 'bigLeft',
    label: 'collage.tpl.bigLeft',
    kind: 'grid',
    make: (n) => {
      const big = n === 2 ? 0.58 : 0.62;
      const m = n - 1;
      const cols = m > 4 ? 2 : 1;
      const right = transpose(rowsLayout(even(m, cols)));
      return [{ x: 0, y: 0, w: big, h: 1 }, ...right.map((c) => ({ x: big + c.x * (1 - big), y: c.y, w: c.w * (1 - big), h: c.h }))];
    },
  },
  {
    id: 'bigTop',
    label: 'collage.tpl.bigTop',
    kind: 'grid',
    make: (n) => {
      const big = n === 2 ? 0.56 : 0.6;
      const m = n - 1;
      const rows = m > 4 ? 2 : 1;
      const bottom = rowsLayout(even(m, rows));
      return [{ x: 0, y: 0, w: 1, h: big }, ...bottom.map((c) => ({ x: c.x, y: big + c.y * (1 - big), w: c.w, h: c.h * (1 - big) }))];
    },
  },
  {
    id: 'rows',
    label: 'collage.tpl.rows',
    kind: 'grid',
    make: (n) => rowsLayout(n <= 4 ? Array.from({ length: n }, () => 1) : even(n, Math.ceil(n / 2))),
  },
  {
    id: 'columns',
    label: 'collage.tpl.columns',
    kind: 'grid',
    make: (n) => transpose(rowsLayout(n <= 4 ? Array.from({ length: n }, () => 1) : even(n, Math.ceil(n / 2)))),
  },
  { id: 'polaroid', label: 'collage.tpl.polaroid', kind: 'free', make: (n, a, s) => scatter(n, a, s, 'polaroid') },
  { id: 'scatter', label: 'collage.tpl.scatter', kind: 'free', make: (n, a, s) => scatter(n, a, s, 'print') },
];

export function templateById(id: string) {
  return TEMPLATES.find((t) => t.id === id) ?? TEMPLATES[0];
}
