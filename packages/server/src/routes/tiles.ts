import path from 'node:path';
import { isTileStyle, TILE_STYLES } from '@peekaround/shared';
import type { Ctx } from '../context.js';
import { getSettings } from '../context.js';
import { TileError, TileService } from '../tiles.js';
import { requireLocal, type Plugin } from './util.js';

/**
 * Map tiles through the server (see tiles.ts):
 *   GET    /api/tiles/:style/:z/:x/:y(.png|.jpg)  → tile image; on failure 502/504 + X-Peek-Tile-Error: offline | refused:<status>
 *   GET    /api/tiles/cache                       → { bytes, files, limitMb }
 *   DELETE /api/tiles/cache                       → clear the cache
 *   GET    /api/tiles/status                      → last upstream problem per style
 * Tiles have their own generous rate limit: panning a map fires dozens of requests per second and
 * must never be throttled like file-system routes (FS_LIMIT / HEAVY_LIMIT don't apply).
 */
export const TILE_LIMIT = { config: { rateLimit: { max: 20_000, timeWindow: '1 minute' } } } as const;

const services = new WeakMap<Ctx, TileService>();

/** the tile service of a server (created on first use; tests replace `upstream`) */
export function tileService(ctx: Ctx): TileService {
  let s = services.get(ctx);
  if (!s) {
    s = new TileService({
      dir: path.join(ctx.cfg.cacheDir, 'tiles'),
      userAgent: `PeekAround/${ctx.cfg.version} (+https://github.com/zearkreast/PeakAround)`,
      capMb: () => getSettings(ctx).tileCacheMb,
      fetch: ctx.cfg.fetch,
    });
    services.set(ctx, s);
  }
  return s;
}

export const tileRoutes: Plugin = (app, ctx) => {
  app.get<{ Params: { style: string; z: string; x: string; y: string } }>('/api/tiles/:style/:z/:x/:y', TILE_LIMIT, async (req, reply) => {
    const { style } = req.params;
    const y = req.params.y.replace(/\.(png|jpe?g)$/i, '');
    const nums = [req.params.z, req.params.x, y];
    if (!isTileStyle(style) || !nums.every((v) => /^\d{1,7}$/.test(v))) {
      return reply.code(400).header('X-Peek-Tile-Error', 'invalid').send({ error: 'Invalid tile' });
    }
    try {
      const t = await tileService(ctx).get(style, Number(req.params.z), Number(req.params.x), Number(y));
      return reply
        .header('Content-Type', t.type)
        .header('Cache-Control', t.cache === 'stale' ? 'no-cache' : 'public, max-age=86400')
        .header('X-Peek-Tile', t.cache)
        .header('Access-Control-Expose-Headers', 'X-Peek-Tile, X-Peek-Tile-Error')
        .send(t.body);
    } catch (e) {
      const err = e instanceof TileError ? e : new TileError('offline', (e as Error).message);
      const code = err.kind === 'invalid' ? 400 : err.kind === 'offline' ? 504 : 502;
      return reply
        .code(code)
        .header('Cache-Control', 'no-store')
        .header('X-Peek-Tile-Error', err.kind === 'refused' ? `refused:${err.status || 0}` : err.kind)
        .header('Access-Control-Expose-Headers', 'X-Peek-Tile, X-Peek-Tile-Error')
        .send({ error: err.message, reason: err.kind, status: err.status || undefined });
    }
  });

  app.get('/api/tiles/cache', async () => tileService(ctx).stats());

  app.delete('/api/tiles/cache', async (req) => {
    requireLocal(req);
    await tileService(ctx).clear();
    return tileService(ctx).stats();
  });

  app.get('/api/tiles/status', async () => {
    const out: Record<string, { reason: string; status: number; at: number } | null> = {};
    for (const s of Object.keys(TILE_STYLES)) {
      const e = tileService(ctx).lastError.get(s as keyof typeof TILE_STYLES);
      out[s] = e ? { reason: e.kind, status: e.status, at: e.at } : null;
    }
    return out;
  });
};
