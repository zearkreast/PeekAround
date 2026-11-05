import { describe, expect, it } from 'vitest';
import { aspectToByte } from '@peekaround/shared';
import { itemAt, layoutGrid, monthGroups, nearestItem, rowAt, type SkeletonDay } from './layout';

function bytes(aspects: number[]) {
  return Uint8Array.from(aspects.map(aspectToByte));
}
const opts = { width: 1000, target: 200, gap: 4, square: false, groupBy: null, headerH: 50 } as const;

describe('grid layout engine', () => {
  it('places every item and fills every full row exactly', () => {
    const a = bytes([1.5, 0.66, 1, 1.78, 1.33, 0.75, 2, 1, 1.5, 0.8, 1, 1]);
    const l = layoutGrid(a, a.length, null, opts);
    let placed = 0;
    for (let r = 0; r < l.rowCount; r++) {
      if (l.rowHeader[r]) continue;
      const s = l.rowStart[r];
      const e = l.rowEnd[r];
      placed += e - s;
      if (r < l.rowCount - 1) expect(l.itemX[e - 1] + l.itemW[e - 1]).toBeCloseTo(1000, 0);
      for (let i = s; i < e; i++) expect(l.itemRow[i]).toBe(r);
    }
    expect(placed).toBe(a.length);
    expect(l.height).toBe(l.rowTop[l.rowCount - 1] + l.rowH[l.rowCount - 1]);
  });

  it('adds a header per day and keeps rows inside their group', () => {
    const days: SkeletonDay[] = [
      { key: '2024-07-03', count: 3, place: 'Kadıköy, İstanbul' },
      { key: '2024-07-01', count: 5, place: null },
      { key: '2024-06-30', count: 1, place: null },
    ];
    const a = bytes(new Array(9).fill(1.5));
    const l = layoutGrid(a, 9, days, { ...opts, groupBy: 'day' });
    expect(l.groups.map((g) => g.key)).toEqual(['2024-07-03', '2024-07-01', '2024-06-30']);
    expect(l.groups[0].place).toBe('Kadıköy, İstanbul');
    expect(l.rowHeader[l.groups[1].row]).toBe(1);
    // items 3..7 are below the second header and above the third
    for (let i = 3; i < 8; i++) {
      expect(l.rowTop[l.itemRow[i]]).toBeGreaterThan(l.groups[1].top);
      expect(l.rowTop[l.itemRow[i]]).toBeLessThan(l.groups[2].top);
    }
    // tops strictly increase
    for (let r = 1; r < l.rowCount; r++) expect(l.rowTop[r]).toBeGreaterThan(l.rowTop[r - 1]);
  });

  it('merges days into months for the compact level', () => {
    const m = monthGroups([
      { key: '2024-07-03', count: 3, place: null },
      { key: '2024-07-01', count: 2, place: 'Bodrum' },
      { key: '2024-06-30', count: 1, place: null },
    ]);
    expect(m).toEqual([
      { key: '2024-07', count: 5, place: 'Bodrum' },
      { key: '2024-06', count: 1, place: null },
    ]);
  });

  it('square mode uses uniform columns', () => {
    const l = layoutGrid(bytes(new Array(10).fill(0.5)), 10, null, { ...opts, square: true, target: 240 });
    expect(l.rowEnd[0] - l.rowStart[0]).toBe(4);
    expect(l.itemX[3] + l.itemW[3]).toBe(1000);
  });

  it('hit testing: rowAt / itemAt / nearestItem', () => {
    const l = layoutGrid(bytes(new Array(20).fill(1)), 20, null, opts);
    const r = rowAt(l, l.rowTop[2] + 5);
    expect(r).toBe(2);
    const i = itemAt(l, l.itemX[l.rowStart[2]] + 2, l.rowTop[2] + 5);
    expect(i).toBe(l.rowStart[2]);
    expect(itemAt(l, 5, -10)).toBe(-1);
    expect(nearestItem(l, 99999, 99999999)).toBe(19);
  });

  it('lays out 100k items in a few milliseconds', () => {
    const n = 100_000;
    const a = new Uint8Array(n);
    for (let i = 0; i < n; i++) a[i] = (i * 37) % 255;
    const days: SkeletonDay[] = [];
    for (let left = n, d = 0; left > 0; d++) {
      const c = Math.min(left, 1 + (d % 40));
      days.push({ key: `20${String(10 + (d % 15)).padStart(2, '0')}-0${1 + (d % 9)}-1${d % 9}`, count: c, place: null });
      left -= c;
    }
    layoutGrid(a, n, days, { ...opts, groupBy: 'day' }); // warm up the JIT
    const t0 = performance.now();
    const l = layoutGrid(a, n, days, { ...opts, groupBy: 'day' });
    const ms = performance.now() - t0;
    expect(l.total).toBe(n);
    expect(l.height).toBeGreaterThan(0);
    // generous bound for slow CI machines; typically 5–15 ms
    expect(ms).toBeLessThan(150);
  });
});
