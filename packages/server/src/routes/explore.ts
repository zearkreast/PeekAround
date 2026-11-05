import type { ExploreResult } from '@peekaround/shared';
import { parseBrowseQuery } from './browse.js';
import type { Plugin } from './util.js';

/**
 * GET /api/explore[?libraryId=&folderId=&dir=&kind=…] → ExploreResult (memories, places, types, cameras, years, indexing).
 * Without parameters it covers every non-temporary folder. /api/geo/points lives in browse.ts (same filters as browse).
 */
export const exploreRoutes: Plugin = (app, ctx) => {
  app.get<{ Querystring: Record<string, string> }>('/api/explore', async (req): Promise<ExploreResult> => {
    const q = parseBrowseQuery(req.query);
    // only location + kind matter for Explore (it IS the grouping of everything inside)
    return ctx.explore.get({ libraryId: q.libraryId, folderId: q.folderId, dir: q.dir, kind: req.query.kind ? q.kind : undefined });
  });
};
