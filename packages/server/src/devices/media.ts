import type { FastifyReply, FastifyRequest } from 'fastify';
import type { MediaDetails } from '@peekaround/shared';
import type { Ctx } from '../context.js';
import type { FolderRow } from '../library.js';
import { toItem, type MediaRow } from '../media/mapper.js';
import { applyExif, GROUP_SET_SQL, groupColumns, groupParams, TIME_SRC, type ExtractedMeta } from '../media/metadata.js';
import { HttpError } from '../paths.js';
import type { TunnelReply } from './hub.js';

const MIME: Record<string, string> = {
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  jfif: 'image/jpeg',
  png: 'image/png',
  gif: 'image/gif',
  webp: 'image/webp',
  avif: 'image/avif',
  bmp: 'image/bmp',
  svg: 'image/svg+xml',
  heic: 'image/heic',
  heif: 'image/heif',
  tif: 'image/tiff',
  tiff: 'image/tiff',
  ico: 'image/x-icon',
  mp4: 'video/mp4',
  m4v: 'video/mp4',
  mov: 'video/quicktime',
  webm: 'video/webm',
  mkv: 'video/x-matroska',
  avi: 'video/x-msvideo',
  '3gp': 'video/3gpp',
  ogv: 'video/ogg',
  mp3: 'audio/mpeg',
  m4a: 'audio/mp4',
  aac: 'audio/aac',
  ogg: 'audio/ogg',
  opus: 'audio/opus',
  wav: 'audio/wav',
  flac: 'audio/flac',
  zip: 'application/zip',
};

export function mimeOf(ext: string): string {
  return MIME[ext] ?? 'application/octet-stream';
}

export function relOfRow(r: Pick<MediaRow, 'dir' | 'name'>): string {
  return r.dir ? `${r.dir}/${r.name}` : r.name;
}

/** Accept exactly one byte range ("bytes=a-b", "bytes=a-", "bytes=-n"); anything else → whole file. */
export function singleRange(h: unknown): string | undefined {
  if (typeof h !== 'string') return undefined;
  const m = /^bytes=(\d*)-(\d*)$/.exec(h.trim());
  if (!m || (m[1] === '' && m[2] === '')) return undefined;
  return `bytes=${m[1]}-${m[2]}`;
}

/** Abort signal that fires when the HTTP client goes away before the response finished. */
export function clientGone(reply: FastifyReply): AbortSignal {
  const ctrl = new AbortController();
  reply.raw.on('close', () => {
    if (!reply.raw.writableFinished) ctrl.abort();
  });
  return ctrl.signal;
}

export function requireOnline(ctx: Ctx, folder: FolderRow): { deviceId: string; key: string } {
  const d = ctx.devices.folderOf(folder);
  if (!ctx.devices.isOnline(d.deviceId)) throw new HttpError(503, 'device offline');
  return d;
}

/**
 * GET /api/media/:id/file for device media: Range → `file` op with `range` → 206 / Content-Range /
 * Content-Length / Accept-Ranges, streamed straight from the reply PUT (seekable <video>).
 */
export async function serveDeviceFile(ctx: Ctx, r: MediaRow, folder: FolderRow, req: FastifyRequest, reply: FastifyReply, download: boolean) {
  const { deviceId, key } = requireOnline(ctx, folder);
  reply.header('Accept-Ranges', 'bytes');
  reply.header('Cache-Control', 'private, max-age=3600');
  if (download) reply.header('Content-Disposition', contentDisposition(r.name));
  if (req.method === 'HEAD') {
    // answered from the index (Fastify's HEAD handling would drain a whole tunnelled stream)
    reply.header('Content-Type', mimeOf(r.ext));
    reply.header('Content-Length', String(r.size));
    reply.hijack();
    reply.raw.writeHead(200, reply.getHeaders() as Record<string, string>);
    reply.raw.end();
    return;
  }
  const range = singleRange(req.headers.range);
  const tr: TunnelReply = await ctx.devices.request(deviceId, 'file', { key, p: relOfRow(r), ...(range ? { range } : {}) }, { prio: 1, signal: clientGone(reply) });
  const status = [200, 206, 416].includes(tr.status) ? tr.status : 502;
  const total = tr.headers['x-peek-total'];
  reply.code(status);
  if (status === 416) {
    tr.body.resume();
    reply.header('Content-Range', tr.headers['x-peek-content-range'] ?? `bytes */${total ?? r.size}`);
    return reply.send();
  }
  if (status === 502) {
    tr.body.destroy();
    throw new HttpError(502, `The device answered ${tr.status}`);
  }
  reply.header('Content-Type', tr.headers['content-type'] || mimeOf(r.ext));
  if (tr.headers['content-length']) reply.header('Content-Length', tr.headers['content-length']);
  if (status === 206 && tr.headers['x-peek-content-range']) reply.header('Content-Range', tr.headers['x-peek-content-range']);
  const mt = Number(tr.headers['x-peek-mtime']);
  if (mt > 0) reply.header('Last-Modified', new Date(mt).toUTCString());
  return reply.send(tr.body);
}

/** Viewer info for device media: stored row + the phone's `details` (when online), which also refines the row. */
export async function deviceDetails(ctx: Ctx, r: MediaRow, folder: FolderRow): Promise<MediaDetails> {
  let d = null;
  try {
    d = await ctx.devices.details(r, folder);
  } catch {
    d = null;
  }
  let row = r;
  if (d) {
    const m: ExtractedMeta = {
      width: r.width,
      height: r.height,
      duration: r.duration,
      playback: r.playback,
      extra: null,
      local: null,
      offset: null,
      instant: r.time_src === TIME_SRC.instant ? r.taken_at : null,
      lat: null,
      lon: null,
      make: null,
      model: null,
      lens: null,
      gpano: false,
      pages: null,
    };
    const ex: Record<string, unknown> = { ...(d.exif ?? {}) };
    if (d.gps && Number.isFinite(d.gps.lat) && Number.isFinite(d.gps.lon)) {
      ex.latitude = d.gps.lat;
      ex.longitude = d.gps.lon;
    }
    if (d.make) ex.Make = d.make;
    if (d.model) ex.Model = d.model;
    if (d.lens) ex.LensModel = d.lens;
    applyExif(m, ex);
    if (!m.width) [m.width, m.height] = [r.width, r.height];
    const g = groupColumns(r, m, ctx.geo, ctx.scanner.homeTimeZone);
    const before = JSON.stringify([r.taken_local, r.lat, r.lon, r.make, r.model, r.flags]);
    ctx.db.run(`UPDATE media SET ${GROUP_SET_SQL} WHERE id = ?`, ...groupParams(g), r.id);
    row = ctx.db.get<MediaRow>('SELECT * FROM media WHERE id = ?', r.id) ?? r;
    if (before !== JSON.stringify([row.taken_local, row.lat, row.lon, row.make, row.model, row.flags])) {
      ctx.events.mediaChanged(String(r.folder_id), r.dir, 'meta');
      ctx.scanner.onGroupsChanged?.();
    }
  }
  const exif: Record<string, string | number | null> = {};
  for (const [k, v] of Object.entries(d?.exif ?? {})) {
    if (v === null || typeof v === 'number') exif[k] = v;
    else if (typeof v === 'string') exif[k] = v.slice(0, 120);
  }
  const deviceName = ctx.lib.folderRef(folder).device?.name ?? '';
  return {
    ...toItem(row),
    path: [deviceName, folder.name, relOfRow(row)].filter(Boolean).join('/'),
    exif: Object.keys(exif).length ? exif : null,
    gps: row.lat != null && row.lon != null ? { lat: row.lat, lon: row.lon } : null,
    place: ctx.geo.placeInfo(row.city, row.cc, row.admin1),
    camera: row.make || row.model || row.lens ? { make: row.make, model: row.model, lens: row.lens } : null,
    tzOffset: row.tz_off ?? null,
  };
}

/** ffmpeg input args reading the device file through our own ranged /file endpoint (seekable). */
export function deviceFfmpegInput(ctx: Ctx, r: MediaRow): string[] {
  if (!ctx.devices.selfUrl) throw new HttpError(501, 'Transcoding device videos is not available');
  return ['-headers', `X-Peek-Internal: ${ctx.devices.internalToken}\r\n`, '-i', `${ctx.devices.selfUrl}/api/media/${r.id}/file`];
}

export function contentDisposition(name: string): string {
  const ascii = name.replace(/[^\x20-\x7e]/g, '_').replace(/"/g, '');
  return `attachment; filename="${ascii}"; filename*=UTF-8''${encodeURIComponent(name)}`;
}
