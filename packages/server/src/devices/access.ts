import type { FastifyRequest } from 'fastify';
import type { Ctx } from '../context.js';
import { HttpError } from '../paths.js';
import { deviceHeader } from './hub.js';

/**
 * Who may see linked-phone folders:
 *  - server mode (Docker): every client (there is no local window; all clients are equally trusted);
 *  - desktop: the local window (requests that did not come through the LAN server) and paired
 *    phones authenticating with `Authorization: Bearer <token>` + `X-Peek-Device`
 *    (or `?pdev=&ptok=` on media URLs, see deviceAuthOf);
 *    other LAN clients see no device folders at all;
 *  - a phone never sees its own folders (`X-Peek-Device` = that device).
 * Requests carrying the hub's internal token (ffmpeg reading /file over loopback) see everything.
 */
export interface Viewer {
  devices: boolean;
  self: string | null;
}

type HeaderBag = { headers: Record<string, unknown> };

/**
 * Where a request's device credentials come from. Headers always win
 * (`Authorization: Bearer <token>` + `X-Peek-Device`); only when neither header is present,
 * `?pdev=<deviceId>&ptok=<token>` is accepted instead — media URLs used by <img>/<video> on a
 * linked phone cannot carry headers. The token is still verified by the hub (sha256 +
 * constant-time compare), so the query form grants exactly what the headers would.
 */
export function deviceAuthOf(req: { headers: Record<string, unknown>; query?: unknown }): HeaderBag {
  const h = req.headers;
  if (h['x-peek-device'] !== undefined || h.authorization !== undefined) return req;
  const q = (req.query ?? {}) as Record<string, unknown>;
  const dev = q.pdev;
  const tok = q.ptok;
  if (typeof dev !== 'string' || typeof tok !== 'string' || !dev || !tok) return req;
  return { headers: { 'x-peek-device': dev, authorization: `Bearer ${tok}` } };
}

export function isLanRequest(req: { raw: object }): boolean {
  return !!(req.raw as { peekLan?: boolean }).peekLan;
}

export function viewerOf(ctx: Ctx, req: FastifyRequest): Viewer {
  const auth = deviceAuthOf(req);
  const self = deviceHeader(auth);
  if (req.headers['x-peek-internal'] === ctx.devices.internalToken) return { devices: true, self: null };
  if (ctx.cfg.mode !== 'desktop' || !isLanRequest(req)) return { devices: true, self };
  return { devices: !!ctx.devices.pairedDevice(auth), self };
}

/**
 * Pairing approval / forgetting: on the desktop only the local window may decide (LAN clients
 * cannot approve themselves in); a server without a window (Docker) accepts any web client.
 * A device can never approve itself.
 */
export function requireApprover(ctx: Ctx, req: FastifyRequest, deviceId?: string) {
  if (ctx.cfg.mode === 'desktop' && isLanRequest(req)) throw new HttpError(403, 'Pairing can only be approved on the PC');
  if (deviceId && deviceHeader(deviceAuthOf(req)) === deviceId) throw new HttpError(403, 'A device cannot approve itself');
}

export function canSeePairing(ctx: Ctx, req: FastifyRequest): boolean {
  return ctx.cfg.mode !== 'desktop' || !isLanRequest(req);
}

/** folder ids / media ids a request refers to (query, params, JSON body) */
function referenced(req: FastifyRequest): { folders: number[]; media: number[] } {
  const folders: number[] = [];
  const media: number[] = [];
  const add = (arr: number[], v: unknown) => {
    const n = Number(v);
    if (v !== undefined && v !== null && v !== '' && Number.isFinite(n)) arr.push(n);
  };
  const q = (req.query ?? {}) as Record<string, unknown>;
  add(folders, q.folderId);
  add(media, q.sourceId);
  if (typeof q.ids === 'string') for (const id of q.ids.split(',')) add(media, id);
  const url = req.routeOptions?.url ?? '';
  const params = (req.params ?? {}) as Record<string, unknown>;
  if (url.startsWith('/api/media/:id')) add(media, params.id);
  const b = req.body as any;
  if (b && typeof b === 'object' && !Buffer.isBuffer(b)) {
    add(folders, b.folderId);
    add(folders, b.to?.folderId);
    add(folders, b.target?.folderId);
    if (Array.isArray(b.refs)) for (const r of b.refs) add(folders, r?.folderId);
    add(media, b.id);
    add(media, b.sourceId);
    add(media, b.audioId);
    if (Array.isArray(b.ids)) for (const id of b.ids) add(media, id);
    if (Array.isArray(b.items)) for (const it of b.items) add(media, it?.id);
  }
  return { folders, media };
}

/** preHandler: hide device folders / media from viewers that may not see them (404, as if absent). */
export function deviceAccessHook(ctx: Ctx) {
  return async (req: FastifyRequest) => {
    if (!ctx.devices.hasFolders()) return;
    if (!req.url.startsWith('/api/') || req.url.startsWith('/api/devices/')) return;
    const v = viewerOf(ctx, req);
    if (v.devices && !v.self) return;
    const { folders, media } = referenced(req);
    const denied = (owner: string | null) => owner !== null && (!v.devices || owner === v.self);
    for (const f of folders) if (denied(ctx.devices.folderOwner(f))) throw new HttpError(404, 'Folder not found');
    for (const m of media) {
      const r = ctx.db.get<{ folder_id: number }>('SELECT folder_id FROM media WHERE id = ?', m);
      if (r && denied(ctx.devices.folderOwner(r.folder_id))) throw new HttpError(404, 'Media not found');
    }
  };
}
