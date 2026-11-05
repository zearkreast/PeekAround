import { HttpBackend } from './http';
import type { Backend } from './types';
import { isNative } from './android/plugin';

/**
 * Decide which backend the UI starts with.
 * - Web / Docker / Electron: the server that served this page (same origin).
 * - Android: what the user picked on the setup screen (phone storage or a server) — null means "show setup".
 */
export async function createInitialBackend(): Promise<Backend | null> {
  if (isNative()) {
    const { createAndroidBackend } = await import('./android-entry');
    return createAndroidBackend();
  }
  return new HttpBackend('');
}
