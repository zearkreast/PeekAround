/**
 * Dominant colour buckets from a thumbhash (pure). The thumbhash header stores the image's average
 * colour (LPQ), so no decoding of the placeholder image is needed — cheap enough for 100k rows.
 */

export type ColorBucket = 'red' | 'orange' | 'yellow' | 'green' | 'blue' | 'purple' | 'pink' | 'bw';

/** stable order (also the numeric value stored in media.color; -1 = no colour / unknown) */
export const COLOR_BUCKETS: readonly ColorBucket[] = ['red', 'orange', 'yellow', 'green', 'blue', 'purple', 'pink', 'bw'];

/** CSS swatch per bucket (UI chips) */
export const COLOR_SWATCH: Record<ColorBucket, string> = {
  red: '#e5484d',
  orange: '#f76b15',
  yellow: '#ffc53d',
  green: '#46a758',
  blue: '#3e63dd',
  purple: '#8e4ec6',
  pink: '#e93d82',
  bw: '#8b8d98',
};

export function isColorBucket(s: unknown): s is ColorBucket {
  return typeof s === 'string' && (COLOR_BUCKETS as readonly string[]).includes(s);
}

const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
const B64_INDEX = (() => {
  const m = new Int16Array(128).fill(-1);
  for (let i = 0; i < 64; i++) m[B64.charCodeAt(i)] = i;
  m['-'.charCodeAt(0)] = 62; // url-safe alphabet
  m['_'.charCodeAt(0)] = 63;
  return m;
})();

/** first `n` bytes of a base64 string (no atob / Buffer dependency) */
export function base64Head(s: string, n: number): Uint8Array | null {
  const out = new Uint8Array(n);
  let bits = 0;
  let acc = 0;
  let k = 0;
  for (let i = 0; i < s.length && k < n; i++) {
    const c = s.charCodeAt(i);
    if (c === 61) break; // '='
    const v = c < 128 ? B64_INDEX[c] : -1;
    if (v < 0) return null;
    acc = (acc << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[k++] = (acc >> bits) & 0xff;
    }
  }
  return k === n ? out : null;
}

/** average colour stored in a thumbhash (same maths as thumbhash's thumbHashToAverageRGBA), 0..1 */
export function thumbhashAverage(hash: Uint8Array): { r: number; g: number; b: number; a: number } | null {
  if (hash.length < 6) return null;
  const header = hash[0] | (hash[1] << 8) | (hash[2] << 16);
  const l = (header & 63) / 63;
  const p = ((header >> 6) & 63) / 31.5 - 1;
  const q = ((header >> 12) & 63) / 31.5 - 1;
  const hasAlpha = header >> 23;
  const a = hasAlpha ? (hash[5] & 15) / 15 : 1;
  const b = l - (2 / 3) * p;
  const r = (3 * l - b + q) / 2;
  const g = r - q;
  const c = (v: number) => Math.max(0, Math.min(1, v));
  return { r: c(r), g: c(g), b: c(b), a };
}

/** rgb 0..1 → hue (0..360), saturation, lightness (0..1) */
export function rgbToHsl(r: number, g: number, b: number): { h: number; s: number; l: number } {
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  if (d < 1e-6) return { h: 0, s: 0, l };
  const s = d / (1 - Math.abs(2 * l - 1));
  let h: number;
  if (max === r) h = ((g - b) / d) % 6;
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  h *= 60;
  if (h < 0) h += 360;
  return { h, s: Math.min(1, s), l };
}

/**
 * Bucket of an (average) colour. Averages of photos are muted, so the saturation threshold is low;
 * near-black, near-white and greyish colours are 'bw'.
 */
export function colorBucketOf(r: number, g: number, b: number): ColorBucket {
  const { h, s, l } = rgbToHsl(r, g, b);
  // chroma relative to the lightness range that can hold it
  const chroma = Math.max(r, g, b) - Math.min(r, g, b);
  if (l < 0.1 || l > 0.94 || chroma < 0.07 || s < 0.12) return 'bw';
  if (h < 12 || h >= 345) return 'red';
  if (h < 42) return 'orange';
  if (h < 68) return 'yellow';
  if (h < 165) return 'green';
  if (h < 255) return 'blue';
  if (h < 290) return 'purple';
  return 'pink';
}

/** media.color value (index into COLOR_BUCKETS) of a base64 thumbhash; -1 when it can't be read */
export function colorIndexOfThumbhash(b64: string | null | undefined): number {
  if (!b64) return -1;
  const h = base64Head(b64, 6);
  const avg = h ? thumbhashAverage(h) : null;
  if (!avg) return -1;
  return COLOR_BUCKETS.indexOf(colorBucketOf(avg.r, avg.g, avg.b));
}
