import fsp from 'node:fs/promises';
import path from 'node:path';
import type { FastifyInstance } from 'fastify';
import type { Ctx } from '../context.js';
import { isDeviceFolder, isRemoteFolder, type FolderRow } from '../library.js';
import type { MediaRow } from '../media/mapper.js';
import { HttpError, resolveInRoot } from '../paths.js';

export function mediaRow(ctx: Ctx, id: string | number): MediaRow {
  const r = ctx.db.get<MediaRow>('SELECT * FROM media WHERE id = ?', Number(id));
  if (!r) throw new HttpError(404, 'Media not found');
  return r;
}

export const DEVICE_READONLY = 'Files on a linked device can only be viewed and copied';

/**
 * Device folders (linked phones) have no host path: everything that touches the file system
 * must refuse them. Default-deny: mediaAbs/folderPath throw 403 for device folders, routes that
 * can serve device media use mediaSource() and branch on `device`.
 */
export function requireLocalFolder(folder: FolderRow, message = DEVICE_READONLY) {
  if (isDeviceFolder(folder)) throw new HttpError(403, message);
  // remote (MEGA streaming) folders: same default-deny — they have no host path and are read-only
  if (isRemoteFolder(folder)) throw new HttpError(403, REMOTE_READONLY);
}

export const REMOTE_READONLY = 'Files in a MEGA link can only be viewed and copied to this computer';

/** Local file of a media row (403 for device media — use mediaSource for read paths that support devices). */
export function mediaAbs(ctx: Ctx, r: MediaRow): { folder: FolderRow; abs: string } {
  const folder = ctx.lib.folderRow(r.folder_id);
  requireLocalFolder(folder);
  return { folder, abs: ctx.scanner.abs(folder, r.dir, r.name) };
}

/**
 * Like mediaAbs but also for device and remote media: abs is then the pseudo path
 * `device://<id>/<key>/<rel>` or `remote://mega/<linkId>/<rel>` (no key).
 */
export function mediaSource(ctx: Ctx, r: MediaRow): { folder: FolderRow; abs: string; device: boolean; remote: boolean } {
  const folder = ctx.lib.folderRow(r.folder_id);
  return { folder, abs: ctx.scanner.abs(folder, r.dir, r.name), device: isDeviceFolder(folder), remote: isRemoteFolder(folder) };
}

export function folderPath(ctx: Ctx, folderId: string | number, rel: string): { folder: FolderRow; abs: string } {
  const folder = ctx.lib.folderRow(folderId);
  requireLocalFolder(folder);
  return { folder, abs: resolveInRoot(folder.path, rel) };
}

export async function moveFile(src: string, dest: string) {
  await fsp.mkdir(path.dirname(dest), { recursive: true });
  try {
    await fsp.rename(src, dest);
  } catch (e: any) {
    if (e.code !== 'EXDEV') throw e;
    await fsp.cp(src, dest, { recursive: true, errorOnExist: true, force: false });
    await fsp.rm(src, { recursive: true, force: true });
  }
}

export function requireWritable(ctx: Ctx) {
  if (ctx.cfg.readonly) throw new HttpError(403, 'PeekAround is running in read-only mode');
}

/** Desktop "share on LAN": remote clients may browse but never reach the host file system by path. */
export function requireLocal(req: { raw: object }) {
  if ((req.raw as { peekLan?: boolean }).peekLan) throw new HttpError(403, 'Not allowed from the network');
}

export type Plugin = (app: FastifyInstance, ctx: Ctx) => void | Promise<void>;

export function rel(dir: string, name: string) {
  return dir ? `${dir}/${name}` : name;
}
