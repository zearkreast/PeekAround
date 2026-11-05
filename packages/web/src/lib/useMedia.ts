import { useEffect, useState } from 'react';

export function useMediaQuery(q: string): boolean {
  const [m, setM] = useState(() => (typeof window !== 'undefined' ? window.matchMedia(q).matches : false));
  useEffect(() => {
    const mq = window.matchMedia(q);
    const on = () => setM(mq.matches);
    on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  }, [q]);
  return m;
}

/** Phones / narrow windows: sidebar becomes a drawer, grid gets smaller rows. */
export const useIsMobile = () => useMediaQuery('(max-width: 767px)');
