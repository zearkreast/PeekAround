import type { Job, RemoteCopySpec, RemoteFolderCreate, RemoteImportSpec, RemoteInspect } from '@peekaround/shared';
import { FS_LIMIT, HEAVY_LIMIT, isLoopbackIp } from '../security.js';
import { inspectTree, megaError, parseMegaUrl } from '../remote/mega.js';
import { startRemoteImport } from '../remote/import.js';
import { HttpError } from '../paths.js';
import { isLanRequest } from '../devices/access.js';
import { folderPath, requireWritable, type Plugin } from './util.js';

/**
 * Remote links (MEGA).
 *   POST /api/remote/inspect {url}          → RemoteInspect (≤ 20k nodes, `truncated`); 400 not a MEGA link
 *   POST /api/remote/import RemoteImportSpec → Job (type 'transfer', bytes/files progress, cancel via DELETE /api/jobs/:id)
 *
 * Remote (streaming) folders — a link shown in a library without downloading it (remote/folders.ts):
 *   POST   /api/remote/folders RemoteFolderCreate   → RemoteFolderCreated (lists the link first: 400/404/429/502 on failure)
 *   POST   /api/remote/folders/:id/refresh          → { ok } (re-list now; status via FolderRef.remote + 'folders' events)
 *   GET    /api/remote/folders/:id/link             → { url } — the link WITH its key: local clients only
 *   POST   /api/remote/copy RemoteCopySpec          → Job (copy to a local folder; also /api/fs/move copy=true)
 *   DELETE /api/folders/:id                         → removes the link (core routes)
 */
export const remoteRoutes: Plugin = (app, ctx) => {
  app.post<{ Body: { url: string } }>('/api/remote/inspect', HEAVY_LIMIT, async (req, reply): Promise<RemoteInspect> => {
    const link = parseMegaUrl(req.body?.url);
    try {
      return inspectTree(await ctx.mega.load(link));
    } catch (e) {
      const err = megaError(e) as Error & { retryAfter?: number };
      if (err.retryAfter) reply.header('Retry-After', String(err.retryAfter));
      throw err;
    }
  });

  app.post<{ Body: RemoteImportSpec }>('/api/remote/import', HEAVY_LIMIT, async (req): Promise<Job> => startRemoteImport(ctx, req.body ?? ({} as RemoteImportSpec)));

  app.post<{ Body: RemoteFolderCreate }>('/api/remote/folders', HEAVY_LIMIT, async (req, reply) => {
    requireWritable(ctx);
    try {
      return await ctx.remote.create(req.body ?? ({} as RemoteFolderCreate));
    } catch (e) {
      const err = megaError(e) as Error & { retryAfter?: number };
      if (err.retryAfter) reply.header('Retry-After', String(err.retryAfter));
      throw err;
    }
  });

  app.post<{ Params: { id: string } }>('/api/remote/folders/:id/refresh', HEAVY_LIMIT, async (req) => {
    const folder = ctx.lib.folderRow(req.params.id);
    if (!folder.remote) throw new HttpError(400, 'Not a remote folder');
    try {
      await ctx.remote.refresh(folder.id);
    } catch (e) {
      throw megaError(e);
    }
    return { ok: true };
  });

  // the key is a credential: the desktop window / a browser on the server machine only, never LAN clients
  app.get<{ Params: { id: string } }>('/api/remote/folders/:id/link', FS_LIMIT, async (req) => {
    const folder = ctx.lib.folderRow(req.params.id);
    if (!folder.remote) throw new HttpError(400, 'Not a remote folder');
    // server mode: only a browser on the server machine itself — not through a reverse proxy on it
    const proxied = req.headers['x-forwarded-for'] !== undefined || req.headers.forwarded !== undefined;
    const local = ctx.cfg.mode === 'desktop' ? !isLanRequest(req) : isLoopbackIp(req.ip) && !proxied;
    if (!local) throw new HttpError(403, 'The MEGA link can only be shown on this computer');
    return { url: ctx.remote.linkOf(folder) };
  });

  app.post<{ Body: RemoteCopySpec }>('/api/remote/copy', HEAVY_LIMIT, async (req): Promise<Job> => {
    requireWritable(ctx);
    const b = req.body ?? ({} as RemoteCopySpec);
    if (!b.to?.folderId) throw new HttpError(400, 'to.folderId required');
    folderPath(ctx, b.to.folderId, b.to.dir ?? ''); // a local, writable target
    return ctx.remote.copy(b);
  });
};
