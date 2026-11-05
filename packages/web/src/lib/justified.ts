/** Flickr-style justified rows: every row fills the width, images keep their aspect ratio. */

export interface JRow {
  /** index of the first item (into the source array) */
  start: number;
  /** exclusive */
  end: number;
  height: number;
  widths: number[];
  /** x offset of every item */
  xs: number[];
}

export function aspectOf(w: number | null | undefined, h: number | null | undefined): number {
  if (!w || !h) return 1;
  return Math.max(0.42, Math.min(3.2, w / h));
}

export function justify(aspects: number[], width: number, target: number, gap: number, startIndex = 0): JRow[] {
  const rows: JRow[] = [];
  if (width <= 0 || !aspects.length) return rows;
  let rowStart = 0;
  let sum = 0;
  const finish = (end: number, height: number, last: boolean) => {
    const widths: number[] = [];
    const xs: number[] = [];
    let x = 0;
    const n = end - rowStart;
    const available = width - gap * (n - 1);
    let used = 0;
    for (let i = rowStart; i < end; i++) {
      let w = aspects[i] * height;
      if (!last) {
        // distribute rounding so the row fills exactly
        w = i === end - 1 ? available - used : Math.round(w);
        used += w;
      }
      widths.push(w);
      xs.push(x);
      x += w + gap;
    }
    rows.push({ start: rowStart + startIndex, end: end + startIndex, height: Math.round(height), widths, xs });
  };
  for (let i = 0; i < aspects.length; i++) {
    sum += aspects[i];
    const n = i - rowStart + 1;
    const rowWidth = sum * target + gap * (n - 1);
    if (rowWidth >= width) {
      const h = (width - gap * (n - 1)) / sum;
      finish(i + 1, h, false);
      rowStart = i + 1;
      sum = 0;
    }
  }
  if (rowStart < aspects.length) {
    const n = aspects.length - rowStart;
    // last row: don't stretch, but shrink if it would overflow
    const h = Math.min(target, (width - gap * (n - 1)) / sum);
    finish(aspects.length, h, true);
  }
  return rows;
}

/** Uniform square grid with the same row model. */
export function squares(count: number, width: number, target: number, gap: number, startIndex = 0): JRow[] {
  const cols = Math.max(2, Math.round((width + gap) / (target + gap)));
  const size = (width - gap * (cols - 1)) / cols;
  const rows: JRow[] = [];
  for (let s = 0; s < count; s += cols) {
    const e = Math.min(count, s + cols);
    const widths: number[] = [];
    const xs: number[] = [];
    for (let i = s; i < e; i++) {
      widths.push(size);
      xs.push((i - s) * (size + gap));
    }
    rows.push({ start: s + startIndex, end: e + startIndex, height: size, widths, xs });
  }
  return rows;
}
