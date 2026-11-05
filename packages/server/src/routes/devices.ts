import type { DeviceInfo } from '@peekaround/shared';
import { FS_LIMIT } from '../security.js';
import { requireApprover, viewerOf, canSeePairing } from '../devices/access.js';
import { openPull } from '../devices/transfer.js';
import { HttpError } from '../paths.js';
import type { Plugin } from './util.js';

/**
 * Linked phones (docs/link-protocol.md).
 *   GET    /api/devices                  → DeviceInfo[] (paired + pending; [] for viewers that may not see devices)
 *   POST   /api/devices/:id/approve      → { ok } (desktop: local window only; server mode: any web client)
 *   POST   /api/devices/:id/reject       → { ok }
 *   DELETE /api/devices/:id              → { ok } forget (unpair, drop its folders + rows)
 *   GET    /api/devices/connect          WebSocket control channel (phone)
 *   PUT    /api/devices/reply/:rid       reply body (phone, Bearer)
 *   GET    /api/devices/pull/:token      one-time PC file download for `receive` (phone, Bearer)
 */
export const deviceRoutes: Plugin = async (app, ctx) => {
  app.get('/api/devices', async (req): Promise<DeviceInfo[]> => {
    const v = viewerOf(ctx, req);
    if (!v.devices) return [];
    const all = ctx.devices.list(v.self);
    // pending pairing requests (and their codes) only for viewers that may approve them
    return canSeePairing(ctx, req) ? all : all.filter((d) => d.paired);
  });

  app.post<{ Params: { id: string } }>('/api/devices/:id/approve', FS_LIMIT, async (req) => {
    requireApprover(ctx, req, req.params.id);
    ctx.devices.approve(req.params.id);
    return { ok: true };
  });

  app.post<{ Params: { id: string } }>('/api/devices/:id/reject', FS_LIMIT, async (req) => {
    requireApprover(ctx, req, req.params.id);
    ctx.devices.reject(req.params.id);
    return { ok: true };
  });

  app.delete<{ Params: { id: string } }>('/api/devices/:id', FS_LIMIT, async (req) => {
    requireApprover(ctx, req);
    ctx.devices.forget(req.params.id);
    return { ok: true };
  });

  app.get('/api/devices/connect', { websocket: true }, (socket, req) => ctx.devices.attach(socket, req));

  app.get<{ Params: { token: string } }>('/api/devices/pull/:token', async (req, reply) => {
    const deviceId = ctx.devices.authenticate(req);
    const p = await openPull(ctx, req.params.token, deviceId);
    reply.header('Content-Type', 'application/octet-stream');
    reply.header('Content-Length', String(p.size));
    reply.header('Last-Modified', new Date(p.mtime).toUTCString());
    reply.header('X-Peek-Mtime', String(p.mtime));
    reply.header('Cache-Control', 'no-store');
    return reply.send(p.stream);
  });

  // reply bodies are streamed raw into the consumer: no body parsing at all in this scope
  await app.register(async (scope) => {
    scope.removeAllContentTypeParsers();
    scope.addContentTypeParser('*', (_req, _payload, done) => done(null));
    scope.put<{ Params: { rid: string } }>('/api/devices/reply/:rid', async (req, reply) => {
      const deviceId = ctx.devices.authenticate(req);
      if (!/^[A-Za-z0-9_-]{22}$/.test(req.params.rid)) throw new HttpError(410, 'Unknown request');
      const code = await ctx.devices.acceptReply(req.params.rid, deviceId, req.raw);
      if (code === 410) throw new HttpError(410, 'Unknown or expired request');
      if (code === 499) {
        // the consumer went away: tell the phone and drop the connection so it stops sending
        reply.header('Connection', 'close');
        reply.raw.once('finish', () => setImmediate(() => req.raw.socket?.destroy()));
      }
      return reply.code(code).send();
    });
  });
};
