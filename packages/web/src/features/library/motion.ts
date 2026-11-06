import { useEffect, useState } from 'react';
import { usePrefs } from '@/store/prefs';

/** lists / card grids already shown this session (stagger-in only on the first load) */
const shown = new Set<string>();

/**
 * true while a list should play its entrance (first time it appears this session, motion on).
 * Re-visits, back navigation and refetches render instantly.
 */
export function useEnterOnce(key: string, ready = true): boolean {
  const motion = usePrefs((s) => s.motion);
  const [first] = useState(() => !shown.has(key));
  useEffect(() => {
    if (ready) shown.add(key);
  }, [key, ready]);
  return first && motion === 'full' && !prefersReduced();
}

export function prefersReduced(): boolean {
  return typeof window !== 'undefined' && !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
}

/** delay of item i in a staggered entrance (capped so long lists do not crawl) */
export const stagger = (i: number, step = 0.035, max = 12) => Math.min(i, max) * step;
