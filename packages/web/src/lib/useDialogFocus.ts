import { useEffect, useRef } from 'react';

/**
 * Basic dialog focus management: while `open`, move focus into the panel (unless something
 * inside, e.g. an autoFocus input, already has it) and give it back to the opener on close.
 * Attach the returned ref to the panel and give it tabIndex={-1}.
 */
export function useDialogFocus<T extends HTMLElement>(open: boolean) {
  const ref = useRef<T>(null);
  useEffect(() => {
    if (!open) return;
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const raf = requestAnimationFrame(() => {
      const el = ref.current;
      if (el && !el.contains(document.activeElement)) el.focus({ preventScroll: true });
    });
    return () => {
      cancelAnimationFrame(raf);
      if (opener?.isConnected && (!document.activeElement || document.activeElement === document.body || ref.current?.contains(document.activeElement)))
        opener.focus({ preventScroll: true });
    };
  }, [open]);
  return ref;
}
