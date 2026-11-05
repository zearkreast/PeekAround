import fsp from 'node:fs/promises';
import path from 'node:path';
import type { FastifyInstance } from 'fastify';
import type { Ctx } from '../context.js';
import type { FolderRow } from '../library.js';
import type { MediaRow } from '../media/mapper.js';
import { HttpError, resolveInRoot } from '../paths.js';

export function mediaRow(ctx: Ctx, id: string | number): MediaRow {
  const r = ctx.db.get<MediaRow>('SELECT * FROM media WHERE id = ?', Number(id));
  if (!r) throw new HttpError(404, 'Media not found');
  return r;
}

export function mediaAbs(ctx: Ctx, r: MediaRow): { folder: FolderRow; abs: string } {
  const folder = ctx.lib.folderRow(r.folder_id);
  return { folder, abs: ctx.scanner.abs(folder, r.dir, r.name) };
}

export function folderPath(ctx: Ctx, folderId: string | number, rel: string): { folder: FolderRow; abs: string } {
  const folder = ctx.lib.folderRow(folderId);
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
