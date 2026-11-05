import fs from 'node:fs';
import path from 'node:path';
import type { MediaDetails } from '@peekaround/shared';
import yazl from 'yazl';
import type { Ctx } from '../context.js';
import { toItem } from '../media/mapper.js';
import { readExif } from '../media/metadata.js';
import { snapSize } from '../media/thumbs.js';
import { HttpError, resolveInRoot } from '../paths.js';
import { mediaAbs, mediaRow, type Plugin } from './util.js';

const IMMUTABLE = 'public, max-age=31536000, immutable';

export const mediaRoutes: Plugin = (app, ctx) => {
  app.get<{ Params: { id: string } }>('/api/media/:id', async (req): Promise<MediaDetails> => {
    const r = mediaRow(ctx, req.params.id);
    const { abs, folder } = mediaAbs(ctx, r);
    const extra = r.extra ? JSON.parse(r.extra) : null;
    return {
      ...toItem(r),
      path: ctx.cfg.mode === 'desktop' ? abs : path.posix.join(folder.name, r.dir, r.name),
      exif: r.kind === 'image' ? await readExif(abs) : null,
      video:
        r.kind === 'video' && extra
          ? { codec: extra.codec, fps: extra.fps, bitrate: extra.bitrate, audioCodec: extra.audioCodec, rotation: extra.rotation ?? 0 }
          : undefined,
    };
  });

  app.patch<{ Params: { id: string }; Body: { favorite?: boolean } }>('/api/media/:id', async (req) => {
    const r = mediaRow(ctx, req.params.id);
    if (typeof req.body?.favorite === 'boolean') {
      ctx.db.run('UPDATE media SET favorite = ? WHERE id = ?', req.body.favorite ? 1 : 0, r.id);
      ctx.events.mediaChanged(String(r.folder_id), r.dir);
    }
    return toItem(mediaRow(ctx, r.id));
  });

  app.post<{ Body: { ids: string[]; favorite: boolean } }>('/api/media/favorite', async (req) => {
    for (const id of req.body.ids ?? []) {
      const r = mediaRow(ctx, id);
      ctx.db.run('UPDATE media SET favorite = ? WHERE id = ?', req.body.favorite ? 1 : 0, r.id);
      ctx.events.mediaChanged(String(r.folder_id), r.dir);
    }
    return { ok: true };
  });

  app.get<{ Params: { id: string }; Querystring: { s?: string } }>('/api/media/:id/thumb', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    const { abs } = mediaAbs(ctx, r);
    const f = await ctx.thumbs.thumb(abs, r, snapSize(Number(req.query.s) || 320)).catch(() => null);
    if (!f) throw new HttpError(404, 'No thumbnail');
    reply.header('Cache-Control', IMMUTABLE);
    return reply.sendFile(path.basename(f), path.dirname(f), { cacheControl: false });
  });

  /** Original file with HTTP range support. ?download=1 forces a download. */
  app.get<{ Params: { id: string }; Querystring: { download?: string } }>('/api/media/:id/file', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    const { abs } = mediaAbs(ctx, r);
    if (!fs.existsSync(abs)) throw new HttpError(404, 'File missing');
    if (req.query.download) reply.header('Content-Disposition', contentDisposition(r.name));
    reply.header('Cache-Control', 'private, max-age=3600');
    return reply.sendFile(path.basename(abs), path.dirname(abs), { cacheControl: false, dotfiles: 'allow' });
  });

  /** Browser friendly full-size rendition for HEIC/TIFF/… */
  app.get<{ Params: { id: string } }>('/api/media/:id/display', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    const { abs } = mediaAbs(ctx, r);
    if (r.playback === 'direct') return reply.redirect(`/api/media/${r.id}/file`);
    const f = await ctx.thumbs.display(abs, r);
    if (!f) throw new HttpError(415, 'Cannot convert');
    reply.header('Cache-Control', IMMUTABLE);
    return reply.sendFile(path.basename(f), path.dirname(f), { cacheControl: false });
  });

  app.get<{ Params: { id: string } }>('/api/media/:id/preview', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    const { abs } = mediaAbs(ctx, r);
    const f = await ctx.thumbs.preview(abs, r).catch(() => null);
    if (!f) throw new HttpError(404, 'No preview');
    reply.header('Cache-Control', IMMUTABLE);
    return reply.sendFile(path.basename(f), path.dirname(f), { cacheControl: false });
  });

  app.get<{ Params: { id: string }; Querystring: { n?: string; h?: string } }>('/api/media/:id/filmstrip', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    const { abs } = mediaAbs(ctx, r);
    const f = await ctx.thumbs.filmstrip(abs, r, Number(req.query.n) || 12, Number(req.query.h) || 72).catch(() => null);
    if (!f) throw new HttpError(404, 'No filmstrip');
    reply.header('Cache-Control', IMMUTABLE);
    return reply.sendFile(path.basename(f), path.dirname(f), { cacheControl: false });
  });

  /** Live H.264 transcode for videos the browser can't play. ?t=seconds to start at. */
  app.get<{ Params: { id: string }; Querystring: { t?: string } }>('/api/media/:id/stream', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    if (r.kind !== 'video') throw new HttpError(400, 'Not a video');
    if (!ctx.ffmpeg.available) throw new HttpError(501, 'ffmpeg not available');
    const { abs } = mediaAbs(ctx, r);
    const t = Math.max(0, Number(req.query.t) || 0);
    const proc = ctx.ffmpeg.spawn([
      '-loglevel',
      'error',
      ...(t ? ['-ss', t.toFixed(2)] : []),
      '-i',
      abs,
      '-map',
      '0:v:0',
      '-map',
      '0:a:0?',
      '-c:v',
      'libx264',
      '-preset',
      'veryfast',
      '-crf',
      '23',
      '-pix_fmt',
      'yuv420p',
      '-vf',
      "scale='min(1920,iw)':-2",
      '-c:a',
      'aac',
      '-b:a',
      '160k',
      '-ac',
      '2',
      '-movflags',
      'frag_keyframe+empty_moov+default_base_moof',
      '-f',
      'mp4',
      'pipe:1',
    ]);
    const kill = () => {
      if (proc.exitCode === null) proc.kill('SIGKILL');
    };
    req.raw.on('close', kill);
    reply.header('Content-Type', 'video/mp4');
    reply.header('Cache-Control', 'no-store');
    return reply.send(proc.stdout);
  });

  /** Download several media files (or a whole directory) as a zip stream. */
  app.get<{ Querystring: { ids?: string; folderId?: string; dir?: string; name?: string } }>('/api/download', async (req, reply) => {
    const zip = new yazl.ZipFile();
    let count = 0;
    if (req.query.ids) {
      const used = new Set<string>();
      for (const id of req.query.ids.split(',').filter(Boolean)) {
        const r = mediaRow(ctx, id);
        const { abs } = mediaAbs(ctx, r);
        let n = r.name;
        for (let i = 1; used.has(n); i++) n = r.name.replace(/(\.[^.]*)?$/, ` (${i})$1`);
        used.add(n);
        zip.addFile(abs, n, { compress: false });
        count++;
      }
    } else if (req.query.folderId) {
      const folder = ctx.lib.folderRow(req.query.folderId);
      const base = resolveInRoot(folder.path, req.query.dir ?? '');
      const walk = (d: string, rel: string) => {
        for (const e of fs.readdirSync(d, { withFileTypes: true })) {
          if (ctx.scanner.skipName(e.name)) continue;
          const p = path.join(d, e.name);
          const rp = rel ? `${rel}/${e.name}` : e.name;
          if (e.isDirectory()) walk(p, rp);
          else if (e.isFile()) {
            zip.addFile(p, rp, { compress: false });
            count++;
          }
        }
      };
      walk(base, '');
    }
    if (!count) throw new HttpError(400, 'Nothing to download');
    zip.end();
    const name = (req.query.name || 'PeekAround') + '.zip';
    reply.header('Content-Type', 'application/zip');
    reply.header('Content-Disposition', contentDisposition(name));
    return reply.send(zip.outputStream);
  });
};

export function contentDisposition(name: string): string {
  const ascii = name.replace(/[^\x20-\x7e]/g, '_').replace(/"/g, '');
  return `attachment; filename="${ascii}"; filename*=UTF-8''${encodeURIComponent(name)}`;
}
