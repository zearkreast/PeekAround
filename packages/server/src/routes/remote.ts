import type { Job, RemoteImportSpec, RemoteInspect } from '@peekaround/shared';
import { HEAVY_LIMIT } from '../security.js';
import { inspectTree, megaError, parseMegaUrl } from '../remote/mega.js';
import { startRemoteImport } from '../remote/import.js';
import type { Plugin } from './util.js';

/**
 * Remote links (MEGA).
 *   POST /api/remote/inspect {url}          → RemoteInspect (≤ 20k nodes, `truncated`); 400 not a MEGA link
 *   POST /api/remote/import RemoteImportSpec → Job (type 'transfer', bytes/files progress, cancel via DELETE /api/jobs/:id)
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
};
