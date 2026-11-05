import { HttpBackend } from './http';
import type { Backend } from './types';
import { loadMode } from './android/config';

/** Android: the backend chosen on the setup screen (null → show the setup screen). */
export async function createAndroidBackend(): Promise<Backend | null> {
  const m = loadMode();
  if (!m) return null;
  if (m.mode === 'server') return new HttpBackend(m.url, m.name ?? 'PeekAround');
  const { AndroidLocalBackend } = await import('./android/local');
  return new AndroidLocalBackend();
}
