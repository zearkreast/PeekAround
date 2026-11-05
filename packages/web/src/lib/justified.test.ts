import { describe, expect, it } from 'vitest';
import { aspectOf, justify, squares } from './justified';

describe('justified layout', () => {
  it('fills every row except the last exactly', () => {
    const aspects = [1.5, 0.66, 1, 1.78, 1.33, 0.75, 2, 1, 1.5, 0.8];
    const rows = justify(aspects, 1000, 200, 6);
    for (const r of rows.slice(0, -1)) {
      const width = r.widths.reduce((a, b) => a + b, 0) + 6 * (r.widths.length - 1);
      expect(Math.abs(width - 1000)).toBeLessThan(1);
    }
    expect(rows.flatMap((r) => r.widths).length).toBe(aspects.length);
  });

  it('keeps the last row at the target height', () => {
    const rows = justify([1, 1], 1000, 200, 6);
    expect(rows).toHaveLength(1);
    expect(rows[0].height).toBe(200);
  });

  it('clamps extreme aspect ratios', () => {
    expect(aspectOf(10000, 10)).toBeLessThanOrEqual(3.2);
    expect(aspectOf(10, 10000)).toBeGreaterThanOrEqual(0.42);
    expect(aspectOf(null, 100)).toBe(1);
  });

  it('builds uniform square rows', () => {
    const rows = squares(7, 1000, 200, 6);
    expect(rows[0].widths.length).toBeGreaterThan(1);
    expect(rows.reduce((a, r) => a + r.widths.length, 0)).toBe(7);
  });
});
