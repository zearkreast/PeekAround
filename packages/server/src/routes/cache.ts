import { getSettings } from '../context.js';
import { HttpError } from '../paths.js';
import { FS_LIMIT, HEAVY_LIMIT } from '../security.js';
import { tileService } from './tiles.js';
import { requireLocal, type Plugin } from './util.js';

export const CACHE_KINDS = ['thumbs', 'tiles', 'remote'] as const;
export type CacheKind = (typeof CACHE_KINDS)[number];

/**
 * Cache usage per kind (Settings → Performance & cache):
 *   GET    /api/cache/details  → { thumbs: { bytes }, tiles: { bytes, files, limitMb }, remote: { bytes, limitMb } }
 *   DELETE /api/cache/:kind    → clear one cache (thumbnails are regenerated in the background).
 *                                Refused for LAN clients of the desktop app (requireLocal).
 * The older GET/DELETE /api/cache (thumbnails + MEGA together) stays for existing clients.
 */
export const cacheRoutes: Plugin = (app, ctx) => {
  app.get('/api/cache/details', FS_LIMIT, async () => {
    const s = getSettings(ctx);
    const [thumbs, tiles, remote] = await Promise.all([
      ctx.thumbs.sizeBytes(),
      tileService(ctx).stats(),
      ctx.remote.cache.sizeBytes().catch(() => 0),
    ]);
    return {
      thumbs: { bytes: thumbs },
      tiles: { bytes: tiles.bytes, files: tiles.files, limitMb: tiles.limitMb },
      remote: { bytes: remote, limitMb: s.remoteCacheMb },
    };
  });

  app.delete<{ Params: { kind: string } }>('/api/cache/:kind', HEAVY_LIMIT, async (req) => {
    requireLocal(req);
    const kind = req.params.kind as CacheKind;
    if (!CACHE_KINDS.includes(kind)) throw new HttpError(400, 'Unknown cache');
    if (kind === 'thumbs') {
      await ctx.thumbs.clear();
      ctx.db.run('UPDATE media SET thumb_state = 0');
      ctx.scanner.kick();
      ctx.remote.kick();
    } else if (kind === 'tiles') await tileService(ctx).clear();
    else await ctx.remote.cache.clear();
    return { ok: true, kind };
  });
};
