import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { pipeline } from 'node:stream/promises';
import { extOf, type Job, type RemoteImportSpec, type UploadZipMode } from '@peekaround/shared';
import { getSettings, type Ctx } from '../context.js';
import { counter } from '../devices/transfer.js';
import type { FolderRow } from '../library.js';
import { HttpError, normalizeRel, sanitizeName, toPosix, uniquePath } from '../paths.js';
import { folderPath, requireWritable } from '../routes/util.js';
import { zipToTemp } from '../routes/files.js';
import { extractZip } from '../zip.js';
import { flatten, megaError, parseMegaUrl, type FlatNode, type MegaNode } from './mega.js';

const MAX_IMPORT_NODES = 200_000;

/**
 * POST /api/remote/import → transfer job.
 *  - mode 'extract': into folderId/dir (local, writable folder); a folder link lands in a new
 *    sub folder named after it (unique), the tree below is preserved;
 *  - mode 'temp': a new temp session named after the link.
 * Only selected nodes (and everything below selected folders) are fetched. Each file streams
 * (megajs decrypting, 4 connections) → `.part` → unique name → index; zips per spec.zip.
 */
export function startRemoteImport(ctx: Ctx, spec: RemoteImportSpec): Job {
  requireWritable(ctx);
  const link = parseMegaUrl(spec?.url);
  if (spec.mode !== 'extract' && spec.mode !== 'temp') throw new HttpError(400, "mode must be 'extract' or 'temp'");
  const zipMode: UploadZipMode = resolveZipMode(ctx, spec.zip);
  let target: { folder: FolderRow; abs: string; dir: string } | null = null;
  if (spec.mode === 'extract') {
    if (!spec.folderId) throw new HttpError(400, 'folderId required');
    const t = folderPath(ctx, spec.folderId, spec.dir ?? '');
    target = { ...t, dir: normalizeRel(spec.dir) };
  }
  const selected = Array.isArray(spec.nodes) ? new Set(spec.nodes.map(String)) : null;
  if (selected && !selected.size) throw new HttpError(400, 'Nothing selected');

  return ctx.jobs.enqueue('transfer', 'MEGA', async (jc) => {
    let root: MegaNode;
    try {
      root = await ctx.mega.load(link);
    } catch (e) {
      throw megaError(e);
    }
    jc.job.title = root.name;
    const { nodes } = flatten(root, MAX_IMPORT_NODES);
    const files = nodes.filter((n) => !n.node.directory && (!selected || n.chain.some((id) => selected.has(id))));
    if (!files.length) throw new Error('Nothing to import');
    const totalBytes = files.reduce((a, f) => a + f.node.size, 0);
    jc.transfer({ bytes: { done: 0, total: totalBytes }, files: { done: 0, total: files.length } });

    let folderId: number;
    let baseAbs: string;
    let baseRel: string;
    let tempRow: FolderRow | null = null;
    if (target) {
      folderId = target.folder.id;
      baseAbs = root.directory ? await uniquePath(target.abs, sanitizeName(root.name)) : target.abs;
      await fsp.mkdir(baseAbs, { recursive: true });
      baseRel = toPosix(path.relative(target.folder.path, baseAbs));
    } else {
      const content = path.join(ctx.cfg.tempDir, crypto.randomUUID(), 'content');
      await fsp.mkdir(content, { recursive: true });
      tempRow = ctx.lib.createTempFolder(content, root.name, getSettings(ctx).tempSessionHours);
      folderId = tempRow.id;
      baseAbs = content;
      baseRel = '';
    }
    const folder = ctx.lib.folderRow(folderId);

    // Phase 1 downloads every file (media lands right away); zips wait as `.part` files and are
    // handled in phase 2 per spec.zip. A failing file/zip never stops the others (only a cancel or
    // a MEGA quota error does); failures end the job with "n of m" and no empty folder stays behind.
    let bytes = 0;
    let done = 0;
    let landed = 0;
    let zipSessions = 0;
    let lastSession: { id: string } | null = null;
    const failures: unknown[] = [];
    const zips: { part: string; dirAbs: string; name: string }[] = [];
    const progress = () => jc.transfer({ bytes: { done: bytes, total: totalBytes }, files: { done, total: files.length } });
    try {
      for (let i = 0; i < files.length; i++) {
        const f = files[i];
        if (jc.signal.aborted) throw new Error('canceled');
        const before = bytes;
        try {
          const got = await downloadOne(f, baseAbs, jc.signal, (n) => {
            bytes += n;
            progress();
          });
          if (zipMode !== 'keep' && extOf(got.name) === 'zip') zips.push(got);
          else {
            await landFile(ctx, folder, got);
            landed++;
          }
        } catch (e) {
          if (jc.signal.aborted) throw e;
          const err = megaError(e);
          failures.push(err);
          // quota: every further request fails the same way → count the rest as failed
          if (err.statusCode === 429) {
            for (let k = i + 1; k < files.length; k++) failures.push(err);
            done = files.length;
            break;
          }
        }
        bytes = before + f.node.size;
        done++;
        progress();
      }
      for (const z of zips) {
        if (jc.signal.aborted) throw new Error('canceled');
        try {
          if (zipMode === 'temp') {
            lastSession = await zipToTemp(ctx, z.part, z.name);
            zipSessions++;
          } else {
            await extractInto(ctx, folder, z.dirAbs, z.part, z.name);
            landed++;
          }
        } catch (e) {
          failures.push(e);
        } finally {
          await fsp.rm(z.part, { force: true });
        }
      }
    } catch (e) {
      await Promise.all(zips.map((z) => fsp.rm(z.part, { force: true })));
      await cleanup();
      if (jc.signal.aborted) throw e;
      throw megaError(e);
    }
    await cleanup();

    let result: Job['result'] = null;
    if (tempRow) result = landed ? { folderId: String(folderId), dir: '', name: root.name } : lastSession ? { folderId: lastSession.id, dir: '', name: '' } : null;
    else if (root.directory) {
      const dir = path.posix.dirname(baseRel) === '.' ? '' : path.posix.dirname(baseRel);
      result = landed ? { folderId: String(folderId), dir, name: path.posix.basename(baseRel) } : { folderId: String(folderId), dir: target!.dir, name: '' };
    } else result = { folderId: String(folderId), dir: target!.dir, name: files.length === 1 && landed ? sanitizeName(files[0].node.name) : '' };
    if (failures.length) {
      jc.job.result = landed || zipSessions ? result : null;
      const first = failures[0] instanceof Error ? failures[0].message : String(failures[0]);
      if (failures.length === files.length && failures.length === 1) throw megaError(failures[0]);
      throw new Error(`${failures.length} of ${files.length} files could not be imported: ${first}`);
    }
    return result;

    /** drop empty folders this import created (an empty temp session as a whole) */
    async function cleanup() {
      if (tempRow && !landed) {
        await ctx.lib.deleteTemp(tempRow, ctx.cfg.tempDir).catch(() => {});
        ctx.events.emit({ type: 'folders' });
        return;
      }
      if (tempRow) ctx.events.emit({ type: 'folders' });
      if (tempRow || root.directory) await pruneEmptyDirs(baseAbs, !tempRow);
    }
  });
}

/** Remove empty directories below `dir` (and `dir` itself when `self`); returns whether `dir` is gone. */
export async function pruneEmptyDirs(dir: string, self: boolean): Promise<boolean> {
  let entries: fs.Dirent[];
  try {
    entries = await fsp.readdir(dir, { withFileTypes: true });
  } catch {
    return false;
  }
  let left = entries.length;
  for (const e of entries) if (e.isDirectory() && !e.isSymbolicLink() && (await pruneEmptyDirs(path.join(dir, e.name), true))) left--;
  if (self && left === 0) {
    await fsp.rmdir(dir).catch(() => {});
    return true;
  }
  return false;
}

function resolveZipMode(ctx: Ctx, z: UploadZipMode | undefined): UploadZipMode {
  if (z === 'extract' || z === 'temp' || z === 'keep') return z;
  const s = getSettings(ctx).zipAction;
  return s === 'extract' || s === 'temp' ? s : 'keep';
}

async function downloadOne(f: FlatNode, baseAbs: string, signal: AbortSignal, onBytes: (n: number) => void) {
  // the root node of a file link has path ''; inside folders the path is relative to the link root
  const segs = (f.path || f.node.name).split('/').map(sanitizeName);
  const name = segs.pop()!;
  const dirAbs = path.join(baseAbs, ...segs);
  await fsp.mkdir(dirAbs, { recursive: true });
  const part = path.join(dirAbs, `${name}.${crypto.randomBytes(4).toString('hex')}.part`);
  try {
    await pipeline(f.node.download({ signal }), counter(onBytes), fs.createWriteStream(part), { signal });
    const ts = f.node.timestamp;
    if (ts && ts > 0 && ts < 4102444800000) await fsp.utimes(part, new Date(), new Date(ts));
  } catch (e) {
    await fsp.rm(part, { force: true });
    throw e;
  }
  return { part, dirAbs, name };
}

async function landFile(ctx: Ctx, folder: FolderRow, f: { part: string; dirAbs: string; name: string }) {
  const final = await uniquePath(f.dirAbs, f.name);
  await fsp.rename(f.part, final);
  await ctx.scanner.upsertFile(folder.id, toPosix(path.relative(folder.path, final)));
}

/** Extract a downloaded zip next to where it would have landed; a failed extraction leaves nothing behind. */
async function extractInto(ctx: Ctx, folder: FolderRow, dirAbs: string, part: string, name: string) {
  const dest = await uniquePath(dirAbs, name.replace(/\.zip$/i, '') || 'Archive');
  try {
    await extractZip(part, dest);
  } catch (e) {
    await fsp.rm(dest, { recursive: true, force: true });
    throw e;
  }
  await ctx.scanner.scanFolder(folder.id, toPosix(path.relative(folder.path, dest)));
}
