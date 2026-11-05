import fs from 'node:fs';
import { FS_LIMIT, HEAVY_LIMIT } from '../security.js';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { MEDIA_FLAG, type MediaDetails } from '@peekaround/shared';
import yazl from 'yazl';
import type { Ctx } from '../context.js';
import type { FolderRow } from '../library.js';
import type { MediaRow } from '../media/mapper.js';
import { toItem } from '../media/mapper.js';
import { readExif } from '../media/metadata.js';
import { snapSize } from '../media/thumbs.js';
import { HttpError, resolveInRoot, trimChars } from '../paths.js';
import { lazyDeviceStream } from '../devices/hub.js';
import { deviceDetails, deviceFfmpegInput, relOfRow, requireOnline, serveDeviceFile } from '../devices/media.js';
import { mediaRow, mediaSource, type Plugin } from './util.js';

const IMMUTABLE = 'public, max-age=31536000, immutable';

export const mediaRoutes: Plugin = (app, ctx) => {
  app.get<{ Params: { id: string } }>('/api/media/:id', FS_LIMIT, async (req): Promise<MediaDetails> => {
    const r = mediaRow(ctx, req.params.id);
    const src = mediaSource(ctx, r);
    if (src.device) return deviceDetails(ctx, r, src.folder);
    const { abs, folder } = src;
    const extra = r.extra ? JSON.parse(r.extra) : null;
    return {
      ...toItem(r),
      path: ctx.cfg.mode === 'desktop' ? abs : path.posix.join(folder.name, r.dir, r.name),
      exif: r.kind === 'image' ? await readExif(abs) : null,
      video:
        r.kind === 'video' && extra
          ? { codec: extra.codec, fps: extra.fps, bitrate: extra.bitrate, audioCodec: extra.audioCodec, rotation: extra.rotation ?? 0 }
          : undefined,
      gps: r.lat != null && r.lon != null ? { lat: r.lat, lon: r.lon } : null,
      place: ctx.geo.placeInfo(r.city, r.cc, r.admin1),
      camera: r.make || r.model || r.lens ? { make: r.make, model: r.model, lens: r.lens } : null,
      tzOffset: r.tz_off ?? null,
    };
  });

  app.patch<{ Params: { id: string }; Body: { favorite?: boolean } }>('/api/media/:id', FS_LIMIT, async (req) => {
    const r = mediaRow(ctx, req.params.id);
    if (typeof req.body?.favorite === 'boolean') {
      ctx.db.run('UPDATE media SET favorite = ? WHERE id = ?', req.body.favorite ? 1 : 0, r.id);
      ctx.events.mediaChanged(String(r.folder_id), r.dir);
    }
    return toItem(mediaRow(ctx, r.id));
  });

  app.post<{ Body: { ids: string[]; favorite: boolean } }>('/api/media/favorite', FS_LIMIT, async (req) => {
    for (const id of req.body.ids ?? []) {
      const r = mediaRow(ctx, id);
      ctx.db.run('UPDATE media SET favorite = ? WHERE id = ?', req.body.favorite ? 1 : 0, r.id);
      ctx.events.mediaChanged(String(r.folder_id), r.dir);
    }
    return { ok: true };
  });

  app.get<{ Params: { id: string }; Querystring: { s?: string } }>('/api/media/:id/thumb', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    // device media: cached thumbnails are served offline too; missing ones come through the tunnel
    const { abs } = mediaSource(ctx, r);
    const f = await ctx.thumbs.thumb(abs, r, snapSize(Number(req.query.s) || 320)).catch(() => null);
    if (!f) throw new HttpError(404, 'No thumbnail');
    reply.header('Cache-Control', IMMUTABLE);
    return reply.sendFile(path.basename(f), path.dirname(f), { cacheControl: false });
  });

  /** Original file with HTTP range support. ?download=1 forces a download. */
  app.get<{ Params: { id: string }; Querystring: { download?: string } }>('/api/media/:id/file', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    const src = mediaSource(ctx, r);
    if (src.device) return serveDeviceFile(ctx, r, src.folder, req, reply, !!req.query.download);
    const { abs } = src;
    if (!(await fsp.access(abs).then(() => true, () => false))) throw new HttpError(404, 'File missing');
    if (req.query.download) reply.header('Content-Disposition', contentDisposition(r.name));
    reply.header('Cache-Control', 'private, max-age=3600');
    return reply.sendFile(path.basename(abs), path.dirname(abs), { cacheControl: false, dotfiles: 'allow' });
  });

  /**
   * Browser friendly rendition. Without `max`: full size for HEIC/TIFF/… (originals the browser can't
   * show), a redirect to the file otherwise. With `max` (px, long edge; snapped to 2048/4096): a bounded
   * cached JPEG/WebP for zooming in the viewer — for every image, so phones never decode 50 MP originals.
   */
  app.get<{ Params: { id: string }; Querystring: { max?: string } }>('/api/media/:id/display', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    if (r.kind !== 'image') throw new HttpError(400, 'Not an image');
    const src = mediaSource(ctx, r);
    const { abs } = src;
    const maxReq = Number(req.query.max) || 0;
    const max = maxReq > 0 ? (maxReq <= 2048 ? 2048 : 4096) : null;
    const animated = r.ext === 'gif' || (r.flags & MEDIA_FLAG.animated) !== 0;
    const fits = max !== null && !!r.width && !!r.height && Math.max(r.width, r.height) <= max;
    if (r.playback === 'direct' && (max === null || fits || animated || r.ext === 'svg')) return reply.redirect(`/api/media/${r.id}/file`);
    // device media: cached renditions work offline; a missing one answers 503 'device offline'
    const f = await ctx.thumbs.display(abs, r, max);
    if (!f) throw new HttpError(415, 'Cannot convert');
    reply.header('Cache-Control', IMMUTABLE);
    return reply.sendFile(path.basename(f), path.dirname(f), { cacheControl: false });
  });

  app.get<{ Params: { id: string } }>('/api/media/:id/preview', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    const src = mediaSource(ctx, r);
    // device videos: the (ranged, tunnelled) file itself is the preview
    if (src.device) {
      if (r.kind !== 'video') throw new HttpError(404, 'No preview');
      return reply.redirect(`/api/media/${r.id}/file`);
    }
    const { abs } = src;
    const f = await ctx.thumbs.preview(abs, r).catch(() => null);
    if (!f) throw new HttpError(404, 'No preview');
    reply.header('Cache-Control', IMMUTABLE);
    return reply.sendFile(path.basename(f), path.dirname(f), { cacheControl: false });
  });

  app.get<{ Params: { id: string }; Querystring: { n?: string; h?: string } }>('/api/media/:id/filmstrip', async (req, reply) => {
    const r = mediaRow(ctx, req.params.id);
    const { abs } = mediaSource(ctx, r);
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
    const src = mediaSource(ctx, r);
    // device videos: ffmpeg reads our own ranged /file endpoint over loopback (seekable, moov-at-end safe)
    if (src.device) requireOnline(ctx, src.folder);
    const input = src.device ? deviceFfmpegInput(ctx, r) : ['-i', src.abs];
    const t = Math.max(0, Number(req.query.t) || 0);
    const proc = ctx.ffmpeg.spawn([
      '-loglevel',
      'error',
      ...(t ? ['-ss', t.toFixed(2)] : []),
      ...input,
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
  const deviceEntry = deviceEntryFactory(ctx);
  app.get<{ Querystring: { ids?: string; folderId?: string; dir?: string; name?: string } }>('/api/download', HEAVY_LIMIT, async (req, reply) => {
    const zip = new yazl.ZipFile();
    let count = 0;
    if (req.query.ids) {
      const used = new Set<string>();
      for (const id of req.query.ids.split(',').filter(Boolean)) {
        const r = mediaRow(ctx, id);
        const src = mediaSource(ctx, r);
        let n = r.name;
        for (let i = 1; used.has(n); i++) n = r.name.replace(/(\.[^.]*)?$/, ` (${i})$1`);
        used.add(n);
        if (src.device) zip.addReadStream(deviceEntry(src.folder, r), n, { compress: false, size: r.size });
        else zip.addFile(src.abs, n, { compress: false });
        count++;
      }
    } else if (req.query.folderId) {
      const folder = ctx.lib.folderRow(req.query.folderId);
      if (folder.device_id) {
        // phone folder: entries from the index, each streamed through the tunnel when the zip reaches it
        const dir = req.query.dir ? trimChars(String(req.query.dir), '/') : '';
        const rows = ctx.db.all<MediaRow>(
          `SELECT * FROM media WHERE folder_id = ?${dir ? ' AND (dir = ? OR substr(dir, 1, ?) = ?)' : ''} ORDER BY dir, name`,
          folder.id,
          ...(dir ? [dir, dir.length + 1, `${dir}/`] : []),
        );
        for (const r of rows) {
          const rel = relOfRow(r);
          zip.addReadStream(deviceEntry(folder, r), dir ? rel.slice(dir.length + 1) : rel, { compress: false, size: r.size });
          count++;
        }
      }
      const base = folder.device_id ? null : resolveInRoot(folder.path, req.query.dir ?? '');
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
      if (base) walk(base, '');
    }
    if (!count) throw new HttpError(400, 'Nothing to download');
    // a failing entry (device went offline…) aborts the download instead of crashing the process
    zip.on('error', (e: Error) => (zip.outputStream as unknown as import('node:stream').Readable).destroy(e));
    zip.end();
    const name = (req.query.name || 'PeekAround') + '.zip';
    reply.header('Content-Type', 'application/zip');
    reply.header('Content-Disposition', contentDisposition(name));
    return reply.send(zip.outputStream);
  });
};

/** zip entry: device file fetched only when the zip writer reaches it */
function deviceEntryFactory(ctx: Ctx) {
  return (folder: FolderRow, r: MediaRow) => {
    const { deviceId, key } = requireOnline(ctx, folder);
    return lazyDeviceStream((signal) => ctx.devices.request(deviceId, 'file', { key, p: relOfRow(r) }, { prio: 1, signal }));
  };
}

export function contentDisposition(name: string): string {
  const ascii = name.replace(/[^\x20-\x7e]/g, '_').replace(/"/g, '');
  return `attachment; filename="${ascii}"; filename*=UTF-8''${encodeURIComponent(name)}`;
}
