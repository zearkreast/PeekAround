import type { SlideshowExportSpec, VideoEditSpec } from '@peekaround/shared';
import { FS_LIMIT, HEAVY_LIMIT } from '../security.js';
import { HttpError } from '../paths.js';
import { requireWritable, type Plugin } from './util.js';
import { registerJobRunners } from '../jobs/runners.js';

/**
 * Video edit + slideshow export jobs.
 * The actual ffmpeg pipelines live in ../jobs/runners.ts (video-edit.ts, slideshow.ts).
 */
export const jobRoutes: Plugin = (app, ctx) => {
  const runners = registerJobRunners(ctx);
  // overlay PNGs are streamed as raw bodies (see PUT /api/jobs/assets)
  app.addContentTypeParser(['application/octet-stream', 'image/png'], (_req, _payload, done) => done(null));

  app.get('/api/jobs', async () => ({ jobs: ctx.jobs.list() }));

  app.delete<{ Params: { id: string } }>('/api/jobs/:id', async (req) => {
    ctx.jobs.cancel(req.params.id);
    return { ok: true };
  });

  app.delete('/api/jobs', async () => {
    ctx.jobs.clearFinished();
    return { jobs: ctx.jobs.list() };
  });

  /** Upload an overlay (transparent PNG rendered by the client) → { assetId } */
  app.put<{ Querystring: { ext?: string } }>('/api/jobs/assets', { ...FS_LIMIT, bodyLimit: 256 * 1024 * 1024 }, async (req) => {
    requireWritable(ctx);
    const id = await ctx.jobs.saveAsset(req.raw as AsyncIterable<Buffer>, req.query.ext || 'png');
    return { assetId: id };
  });

  app.post<{ Body: VideoEditSpec }>('/api/jobs/video-edit', HEAVY_LIMIT, async (req) => {
    requireWritable(ctx);
    if (!ctx.ffmpeg.available) throw new HttpError(501, 'ffmpeg is not available');
    return runners.videoEdit(req.body);
  });

  app.post<{ Body: SlideshowExportSpec }>('/api/jobs/slideshow', HEAVY_LIMIT, async (req) => {
    requireWritable(ctx);
    if (!ctx.ffmpeg.available) throw new HttpError(501, 'ffmpeg is not available');
    return runners.slideshow(req.body);
  });
};
