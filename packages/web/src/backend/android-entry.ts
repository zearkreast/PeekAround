import { HttpBackend } from './http';
import type { Backend } from './types';
import { loadMode } from './android/config';

/** Android: the backend chosen on the setup screen (null → show the setup screen). */
export async function createAndroidBackend(): Promise<Backend | null> {
  const m = loadMode();
  if (!m) return null;
  if (m.mode === 'server') {
    let b: HttpBackend;
    try {
      b = new HttpBackend(m.url, m.name ?? 'PeekAround');
    } catch {
      return null; // stored address is not a valid http(s) URL → back to the setup screen
    }
    // linked phone paired with this server → authenticate (see android/linkAuth.ts)
    const { attachLinkAuth } = await import('./android/linkAuth');
    const { queryClient } = await import('@/lib/queryClient');
    await attachLinkAuth(b, () => {
      void queryClient.invalidateQueries({ queryKey: ['libraries'] });
      void queryClient.invalidateQueries({ queryKey: ['devices'] });
    }).catch(() => {});
    return b;
  }
  const { AndroidLocalBackend } = await import('./android/local');
  return new AndroidLocalBackend();
}
