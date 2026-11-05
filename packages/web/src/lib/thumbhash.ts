import { thumbHashToDataURL } from 'thumbhash';

const cache = new Map<string, string>();

/** base64 thumbhash → tiny blurred PNG data url (memoized). */
export function thumbhashUrl(hash: string | null | undefined): string | undefined {
  if (!hash) return undefined;
  let u = cache.get(hash);
  if (!u) {
    try {
      const bin = Uint8Array.from(atob(hash), (c) => c.charCodeAt(0));
      u = thumbHashToDataURL(bin);
    } catch {
      return undefined;
    }
    if (cache.size > 4000) cache.clear();
    cache.set(hash, u);
  }
  return u;
}
