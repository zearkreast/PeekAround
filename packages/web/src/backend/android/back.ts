/**
 * Android system back (hardware button / gesture, predictive back on targetSdk 36).
 *
 * Native side: PeekBackPlugin.kt registers an androidx OnBackPressedCallback whose
 * `enabled` flag mirrors `canHandleBack()` here. While enabled, the system back is routed
 * to JS as a 'back' plugin event → performBack() closes the top overlay (viewer, sheet,
 * modal, selection, drawer) or goes back in the in-app history. At the root the callback is
 * disabled, so Android's default (move task to back / exit, with the predictive animation)
 * applies.
 */
import { registerPlugin, type PluginListenerHandle } from '@capacitor/core';
import { canHandleBack, onBackStackChange, performBack } from '@/lib/backStack';
import { isNative } from './plugin';

interface PeekBackPlugin {
  setBackEnabled(o: { enabled: boolean }): Promise<void>;
  /** JS had nothing to close after all → let Android perform its default back now */
  fallback(): Promise<void>;
  addListener(event: 'back', fn: () => void): Promise<PluginListenerHandle>;
}

let started = false;

/** Wire the JS back stack to the native callback. Returns a disposer (no-op on the web). */
export function initAndroidBack(): () => void {
  if (started || typeof window === 'undefined' || !isNative()) return () => {};
  started = true;
  const PeekBack = registerPlugin<PeekBackPlugin>('PeekBack');
  let last: boolean | null = null;
  const sync = () => {
    const enabled = canHandleBack();
    if (enabled === last) return;
    last = enabled;
    void PeekBack.setBackEnabled({ enabled }).catch(() => undefined);
  };
  const offStack = onBackStackChange(sync);
  window.addEventListener('popstate', sync);
  sync();
  let handle: PluginListenerHandle | null = null;
  void PeekBack.addListener('back', () => {
    if (!performBack()) {
      // nothing left in JS: hand this back press to the system
      last = false;
      void PeekBack.fallback().catch(() => undefined);
    }
    // history changes are async; re-sync after the router settled
    setTimeout(sync, 50);
  }).then((h) => (handle = h));
  return () => {
    started = false;
    offStack();
    window.removeEventListener('popstate', sync);
    void handle?.remove();
  };
}
