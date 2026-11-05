import type { HttpBackend } from '../http';
import { PeekLink, type LinkStatus } from './link-plugin';

/** "http://192.168.1.20:47213/" and "HTTP://192.168.1.20:47213" are the same server. */
export function normalizeServer(url: string | null | undefined): string {
  if (!url) return '';
  try {
    const u = new URL(url.trim());
    const port = u.port || (u.protocol === 'https:' ? '443' : '80');
    return `${u.protocol}//${u.hostname.toLowerCase()}:${port}`;
  } catch {
    return url.trim().replace(/\/+$/, '').toLowerCase();
  }
}

export function sameServer(a: string | null | undefined, b: string | null | undefined): boolean {
  const x = normalizeServer(a);
  return !!x && x === normalizeServer(b);
}

/** The device credentials to use against `base`, or null (not paired with that server). */
export function authFor(status: Pick<LinkStatus, 'paired' | 'token' | 'deviceId' | 'server'> | null, base: string) {
  if (!status?.paired || !status.token || !status.deviceId) return null;
  return sameServer(status.server, base) ? { deviceId: status.deviceId, token: status.token } : null;
}

/**
 * Android client mode: when this phone shares folders with the SAME server it browses (PeekLink
 * paired), API calls and media URLs carry the device credentials so the PC's other linked devices
 * show up (the server never shows the phone its own folders). Follows pairing changes live.
 */
export async function attachLinkAuth(b: HttpBackend, onChange?: () => void): Promise<() => void> {
  const base = b.base || location.origin;
  let key = '';
  const apply = (s: LinkStatus | null) => {
    const a = authFor(s, base);
    const k = a ? `${a.deviceId}:${a.token}` : '';
    if (k === key) return;
    key = k;
    b.setDeviceAuth(a);
    onChange?.();
  };
  try {
    apply(await PeekLink.status());
  } catch {
    return () => {};
  }
  const h = await PeekLink.addListener('link', (s) => apply(s)).catch(() => null);
  return () => void h?.remove();
}
