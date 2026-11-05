/**
 * Decoded-image cache for the viewer: images are fetched AND decoded off the main
 * paint path (img.decode()) before they are swapped in, so a swipe never shows a
 * half-decoded frame. Small LRU keeps neighbours alive and releases far ones.
 */
const MAX = 10;
const cache = new Map<string, { img: HTMLImageElement; p: Promise<boolean>; ok: boolean }>();

export function decodeImage(url: string): Promise<boolean> {
  const hit = cache.get(url);
  if (hit) {
    cache.delete(url);
    cache.set(url, hit);
    return hit.p;
  }
  const img = new Image();
  img.decoding = 'async';
  const entry = { img, ok: false, p: Promise.resolve(false) };
  entry.p = new Promise<boolean>((resolve) => {
    img.onload = () => {
      img
        .decode()
        .catch(() => undefined)
        .then(() => {
          entry.ok = true;
          resolve(true);
        });
    };
    img.onerror = () => {
      cache.delete(url);
      resolve(false);
    };
  });
  img.src = url;
  cache.set(url, entry);
  while (cache.size > MAX) {
    const first = cache.keys().next().value as string;
    const e = cache.get(first);
    cache.delete(first);
    if (e && !e.ok) e.img.src = '';
  }
  return entry.p;
}

export function isDecoded(url: string): boolean {
  return !!cache.get(url)?.ok;
}

export function _clearImageCache() {
  cache.clear();
}
