/**
 * Shared-element page transitions with the View Transitions API (Chromium / Electron):
 * a card's cover morphs into the header of the page it opens (library card → library banner,
 * album card → album header, folder card → folder header). Elsewhere (or with motion reduced /
 * off) navigation is instant and the regular motion page fade plays.
 *
 *   source:  onClick={(e) => heroNavigate(e, nav, to, `lib-${id}`)}  on a link whose cover has data-hero
 *   target:  <img style={{ viewTransitionName: heroName(`lib-${id}`) }} …>
 */
import { flushSync } from 'react-dom';
import type { NavigateFunction } from 'react-router';
import { usePrefs } from '@/store/prefs';
import { prefersReduced } from './motion';

let pending: string | null = null;
/** cover id the source card showed, per hero key (the target can show it before its own data loads) */
const covers = new Map<string, string>();

export function heroCover(key: string): string | undefined {
  return covers.get(key);
}
let clearTimer: ReturnType<typeof setTimeout> | null = null;

type StartVT = (cb: () => void) => { finished: Promise<void> };

function supported(): StartVT | null {
  const d = document as Document & { startViewTransition?: StartVT };
  if (typeof d.startViewTransition !== 'function') return null;
  if (usePrefs.getState().motion !== 'full' || prefersReduced()) return null;
  return d.startViewTransition.bind(d);
}

/**
 * Props for the element that receives the morph: always `data-hero-target` (heroNavigate waits for it),
 * plus the view-transition-name right after heroNavigate.
 */
export function heroProps(key: string): { 'data-hero-target': string; style: { viewTransitionName?: string } } {
  return { 'data-hero-target': key, style: { viewTransitionName: pending === key ? 'hero' : undefined } };
}

/**
 * Navigate with a cover → header morph. Plain left clicks only (new-tab / modifier clicks keep the
 * browser default). The source element is the `[data-hero]` inside the clicked link.
 */
export function heroNavigate(e: React.MouseEvent, nav: NavigateFunction, to: string, key: string, cover?: string | null): void {
  if (cover) covers.set(key, cover);
  if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
  const start = supported();
  if (!start) return; // the <Link> navigates normally
  const src = (e.currentTarget as HTMLElement).querySelector<HTMLElement>('[data-hero]') ?? (e.currentTarget as HTMLElement);
  e.preventDefault();
  clearNames();
  src.style.viewTransitionName = 'hero';
  try {
    const vt = start(async () => {
      // old snapshot taken: hand the name over to the new page's header
      src.style.viewTransitionName = '';
      pending = key;
      flushSync(() => nav(to));
      // the router renders in a transition: wait (frozen screen, ≤ 350 ms) until the target exists
      await waitFor(`[data-hero-target="${CSS.escape(key)}"]`, 350);
    });
    vt.finished.finally(() => {
      if (clearTimer) clearTimeout(clearTimer);
      clearTimer = setTimeout(() => {
        pending = null;
        clearNames();
      }, 0);
    });
  } catch {
    src.style.viewTransitionName = '';
    pending = null;
    nav(to);
  }
}

/** a later morph must find exactly one source: drop names left on old targets */
function clearNames() {
  for (const el of document.querySelectorAll<HTMLElement>('[style*="view-transition-name"]')) el.style.viewTransitionName = '';
}

function waitFor(selector: string, ms: number): Promise<void> {
  return new Promise((resolve) => {
    if (document.querySelector(selector)) return resolve();
    const done = () => {
      mo.disconnect();
      clearTimeout(timer);
      resolve();
    };
    const mo = new MutationObserver(() => document.querySelector(selector) && done());
    mo.observe(document.body, { childList: true, subtree: true });
    const timer = setTimeout(done, ms);
  });
}
