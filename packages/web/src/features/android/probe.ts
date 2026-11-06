import type { ServerInfo } from '@peekaround/shared';
import { normalizeBase } from '@/lib/safeUrl';

/** the desktop app's default LAN share port (Settings → Network) */
export const LAN_PORT = 1313;
/** the Docker/server default (and the desktop default before 1.3.1) */
export const SERVER_PORT = 8080;

export type ProbeError = 'invalid' | 'unreachable' | 'refused' | 'not-peekaround';
export type ProbeResult = { ok: true; url: string; info: ServerInfo } | { ok: false; error: ProbeError; status?: number };

/**
 * Addresses to try for what was typed on the connect screen: "192.168.1.20:1313", "pc.local" or a full
 * URL. Without a scheme http:// is added; without a port the address as typed (port 80, a reverse
 * proxy), the desktop app's LAN port and the Docker/server port. Returns [] for something that is not an http(s) address.
 */
export function candidateUrls(input: string): string[] {
  let raw = String(input ?? '').trim();
  if (!raw || /^https?:\/\/?$/i.test(raw)) return [];
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(raw)) raw = `http://${raw}`;
  let base: string;
  try {
    base = normalizeBase(raw);
  } catch {
    return [];
  }
  const u = new URL(base);
  if (!u.hostname) return [];
  if (u.port || u.pathname !== '/') return [base];
  const out = [base];
  for (const p of [LAN_PORT, SERVER_PORT]) {
    u.port = String(p);
    out.push(normalizeBase(u.href));
  }
  return out;
}

/**
 * Is there a PeekAround server at `input`? GET /api/info (what 1.1 checked, kept compatible with
 * every server version) with a timeout per address. The error says what to check:
 * 'unreachable' = nothing answered (address, same Wi-Fi, LAN sharing on, PC firewall),
 * 'refused' = it answered with an error status, 'not-peekaround' = some other web server.
 */
export async function probeServer(input: string, opts: { fetch?: typeof fetch; timeoutMs?: number } = {}): Promise<ProbeResult> {
  const doFetch = opts.fetch ?? fetch;
  const urls = candidateUrls(input);
  if (!urls.length) return { ok: false, error: 'invalid' };
  let result: ProbeResult = { ok: false, error: 'unreachable' };
  for (const url of urls) {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), opts.timeoutMs ?? 6000);
    try {
      const res = await doFetch(`${url}/api/info`, { signal: ctrl.signal });
      let info: Partial<ServerInfo> | null = null;
      try {
        info = (await res.json()) as Partial<ServerInfo>;
      } catch {
        info = null;
      }
      if (res.ok && info?.name === 'PeekAround') return { ok: true, url, info: info as ServerInfo };
      // an answer beats "nothing there" when telling the user what went wrong
      result = res.ok ? { ok: false, error: 'not-peekaround' } : { ok: false, error: 'refused', status: res.status };
    } catch {
      /* timeout / network error: try the next address */
    } finally {
      clearTimeout(timer);
    }
  }
  return result;
}
