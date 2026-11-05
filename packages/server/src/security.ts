import os from 'node:os';
import net from 'node:net';
import type { FastifyInstance, FastifyRequest } from 'fastify';
import rateLimit from '@fastify/rate-limit';
import type { Ctx } from './context.js';

/**
 * Browser-facing protection for an unauthenticated LAN gallery:
 *
 *  - Host allow-list  → DNS-rebinding: a hostile site can't point its own domain at 127.0.0.1/LAN IP
 *                        and talk to us as "same origin".
 *  - Origin allow-list → CORS only for our own origins (same origin, the Capacitor app, configured extras);
 *                        state-changing requests and WebSockets from foreign origins are refused (CSRF).
 *  - Rate limiting     → a generous global limit, stricter on file-system mutating routes.
 *
 * Native clients (the Android link service, ffmpeg loopback) send no Origin and pass.
 */

/** the Android app's WebView origin (capacitor.config.json: androidScheme http) */
const APP_ORIGINS = new Set(['http://localhost', 'https://localhost', 'capacitor://localhost']);

/** local-only TLDs / names that a remote attacker cannot own */
const LOCAL_SUFFIXES = ['.local', '.lan', '.home', '.internal', '.home.arpa', '.localhost'];

function splitList(v: string | undefined): string[] {
  return (v ?? '')
    .split(',')
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);
}

export interface SecurityOptions {
  /** extra Host names (reverse proxy domains). '*' disables the Host check. env PEEK_ALLOWED_HOSTS */
  allowedHosts?: string[];
  /** extra browser origins allowed to call the API. env PEEK_ALLOWED_ORIGINS */
  allowedOrigins?: string[];
}

export function securityOptionsFromEnv(): SecurityOptions {
  return { allowedHosts: splitList(process.env.PEEK_ALLOWED_HOSTS), allowedOrigins: splitList(process.env.PEEK_ALLOWED_ORIGINS) };
}

/** "Host" header → bare lower-case host name (no port, no IPv6 brackets) */
export function hostName(host: string | undefined): string {
  if (!host) return '';
  const h = host.trim().toLowerCase();
  if (h.startsWith('[')) {
    const end = h.indexOf(']');
    return end > 0 ? h.slice(1, end) : h;
  }
  const colon = h.indexOf(':');
  return colon >= 0 && h.indexOf(':', colon + 1) < 0 ? h.slice(0, colon) : h;
}

export function isAllowedHost(host: string | undefined, extra: string[] = []): boolean {
  if (extra.includes('*')) return true;
  const name = hostName(host);
  if (!name) return true; // HTTP/1.0 or raw clients without Host: nothing to rebind
  if (net.isIP(name)) return true; // DNS rebinding needs a domain name; raw IPs are fine
  if (name === 'localhost') return true;
  if (!name.includes('.')) return true; // single-label LAN names ("nas", the machine name)
  if (LOCAL_SUFFIXES.some((s) => name.endsWith(s))) return true;
  const hn = os.hostname().toLowerCase();
  if (name === hn || name.startsWith(`${hn}.`)) return true;
  return extra.some((e) => name === e || (e.startsWith('*.') && name.endsWith(e.slice(1))));
}

const LOOPBACK_NAMES = new Set(['localhost', '127.0.0.1', '[::1]', '::1']);

export function isLoopbackIp(ip: string | undefined): boolean {
  return ip === '127.0.0.1' || ip === '::1' || ip === '::ffff:127.0.0.1';
}

/**
 * @param loopbackClient the request comes from this machine: a local dev server
 *   (e.g. Vite on http://localhost:5173 proxying to us) may then call the API.
 */
export function isAllowedOrigin(origin: string | undefined, host: string | undefined, extra: string[] = [], loopbackClient = false): boolean {
  if (!origin) return true; // same-origin navigations, native apps, curl
  if (origin === 'null') return false; // sandboxed iframes, file:// pages
  const o = origin.toLowerCase();
  if (APP_ORIGINS.has(o)) return true;
  if (extra.includes(o) || extra.includes('*')) return true;
  let u: URL;
  try {
    u = new URL(o);
  } catch {
    return false;
  }
  if (u.protocol !== 'http:' && u.protocol !== 'https:') return false;
  if (loopbackClient && LOOPBACK_NAMES.has(u.hostname)) return true;
  // same origin: the page was served by us (host incl. port must match)
  return !!host && u.host === host.toLowerCase();
}

const SAFE_METHODS = new Set(['GET', 'HEAD', 'OPTIONS']);

/** Requests the server makes to itself (ffmpeg reading a device file over loopback). */
function isInternal(req: FastifyRequest): boolean {
  return !!req.headers['x-peek-internal'] && isLoopbackIp(req.ip);
}

export async function registerSecurity(app: FastifyInstance, _ctx: Ctx, opts: SecurityOptions = {}) {
  const hosts = (opts.allowedHosts ?? []).map((h) => h.toLowerCase());
  const origins = (opts.allowedOrigins ?? []).map((o) => o.toLowerCase());

  // Host + Origin checks run before anything else (incl. static files and WebSocket upgrades).
  app.addHook('onRequest', async (req, reply) => {
    if (!isAllowedHost(req.headers.host, hosts)) {
      return reply.code(421).send({ error: 'Unknown host. Add it to PEEK_ALLOWED_HOSTS to allow it.' });
    }
    const origin = req.headers.origin;
    const allowed = isAllowedOrigin(origin, req.headers.host, origins, isLoopbackIp(req.ip));
    const upgrade = String(req.headers.upgrade ?? '').toLowerCase() === 'websocket';
    if (!allowed && (upgrade || !SAFE_METHODS.has(req.method))) {
      return reply.code(403).send({ error: 'Cross-origin request refused' });
    }
    // browsers that don't send Origin on a simple cross-site POST still send Sec-Fetch-Site
    if (!origin && !SAFE_METHODS.has(req.method) && req.headers['sec-fetch-site'] === 'cross-site') {
      return reply.code(403).send({ error: 'Cross-site request refused' });
    }
  });

  await app.register(rateLimit, {
    global: true,
    // galleries fire hundreds of thumbnail requests while scrolling: keep the global limit generous
    max: 6000,
    timeWindow: '1 minute',
    keyGenerator: (req) => req.ip,
    allowList: (req) => isInternal(req) || req.url.startsWith('/api/devices/reply/'),
    errorResponseBuilder: (_req, c) => ({ statusCode: 429, error: `Too many requests, retry in ${Math.ceil(c.ttl / 1000)} s` }),
  });
}

/** Stricter per-route limit for routes that touch the file system in bulk (spread into route options). */
export const FS_LIMIT = { config: { rateLimit: { max: 240, timeWindow: '1 minute' } } } as const;
/** Limit for expensive one-shot work (zip extraction, imports, host directory listings). */
export const HEAVY_LIMIT = { config: { rateLimit: { max: 60, timeWindow: '1 minute' } } } as const;

export function corsDelegator(opts: SecurityOptions = {}) {
  const origins = (opts.allowedOrigins ?? []).map((o) => o.toLowerCase());
  return (req: FastifyRequest, cb: (err: Error | null, o: { origin: boolean; credentials: boolean; methods: string[] }) => void) => {
    cb(null, {
      origin: isAllowedOrigin(req.headers.origin, req.headers.host, origins, isLoopbackIp(req.ip)),
      credentials: false,
      methods: ['GET', 'HEAD', 'PUT', 'POST', 'PATCH', 'DELETE', 'OPTIONS'],
    });
  };
}
