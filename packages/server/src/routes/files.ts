import crypto from 'node:crypto';
import { FS_LIMIT, HEAVY_LIMIT } from '../security.js';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { pipeline } from 'node:stream/promises';
import sharp from 'sharp';
import exifr from 'exifr';
import { extOf, type FileRef, type UploadResult, type UploadZipMode } from '@peekaround/shared';
import { getSettings, type Ctx } from '../context.js';
import { TRASH_DIR } from '../media/scanner.js';
import { HttpError, normalizeRel, resolveHostPath, resolveInRoot, sanitizeName, sanitizeRel, toPosix, uniqueEditPath, uniquePath } from '../paths.js';
import { extractZip } from '../zip.js';
import { copyFromDevice, copyToDevice } from '../devices/transfer.js';
import { folderPath, mediaAbs, mediaRow, moveFile, requireLocal, requireWritable, type Plugin } from './util.js';

function relOf(root: string, abs: string) {
  return toPosix(path.relative(root, abs));
}

function dirOfRel(rel: string) {
  const d = path.posix.dirname(rel);
  return d === '.' ? '' : d;
}

/** Turn an extracted/uploaded zip into a temp session. */
export async function zipToTemp(ctx: Ctx, zipFile: string, sourceName: string) {
  const id = crypto.randomUUID();
  const sessionDir = path.join(ctx.cfg.tempDir, id);
  const content = path.join(sessionDir, 'content');
  try {
    await extractZip(zipFile, content);
  } catch (e) {
    await fsp.rm(sessionDir, { recursive: true, force: true });
    throw e;
  }
  const row = ctx.lib.createTempFolder(content, sourceName, getSettings(ctx).tempSessionHours);
  await ctx.scanner.scanFolder(row.id);
  return ctx.lib.folderRef(row);
}

export const fileRoutes: Plugin = (app, ctx) => {
  // Raw body streaming: we never buffer uploads in memory.
  app.addContentTypeParser('application/octet-stream', (_req, _payload, done) => done(null));
  for (const t of ['image/*', 'video/*', 'audio/*', 'application/zip', 'font/*', 'application/x-zip-compressed']) {
    app.addContentTypeParser(t, (_req, _payload, done) => done(null));
  }

  /**
   * PUT /api/upload?folderId=&dir=&path=sub/dir/name.jpg&zip=extract|temp|keep&mtime=<ms>
   * Body: raw file bytes.
   */
  app.put<{ Querystring: { folderId: string; dir?: string; path: string; zip?: UploadZipMode; mtime?: string } }>(
    '/api/upload',
    { ...FS_LIMIT, bodyLimit: ctx.cfg.maxUploadBytes },
    async (req): Promise<UploadResult> => {
      requireWritable(ctx);
      const { folder } = folderPath(ctx, req.query.folderId, '');
      const baseDir = normalizeRel(req.query.dir);
      const relPath = sanitizeRel(req.query.path || 'upload.bin');
      if (!relPath) throw new HttpError(400, 'Missing path');
      const name = path.posix.basename(relPath);
      const isZip = extOf(name) === 'zip';
      const mode: UploadZipMode = isZip ? (req.query.zip ?? 'keep') : 'keep';

      if (isZip && mode !== 'keep') {
        const scratchDir = path.join(ctx.cfg.tempDir, 'uploads');
        await fsp.mkdir(scratchDir, { recursive: true });
        const scratch = path.join(scratchDir, `${crypto.randomUUID()}.zip`);
        try {
          await pipeline(req.raw, fs.createWriteStream(scratch));
          if (mode === 'temp') {
            const tempFolder = await zipToTemp(ctx, scratch, name);
            return { created: [], tempFolder };
          }
          // extract permanently next to where the zip would have landed
          const parentRel = [baseDir, dirOfRel(relPath)].filter(Boolean).join('/');
          const parentAbs = resolveInRoot(folder.path, parentRel);
          await fsp.mkdir(parentAbs, { recursive: true });
          const dest = await uniquePath(parentAbs, name.replace(/\.zip$/i, '') || 'Archive');
          const res = await extractZip(scratch, dest);
          const destRel = relOf(folder.path, dest);
          await ctx.scanner.scanFolder(folder.id, destRel);
          return {
            created: res.files.map((f) => ({ folderId: String(folder.id), dir: dirOfRel(`${destRel}/${f}`), name: path.posix.basename(f) })),
            extractedDir: destRel,
          };
        } finally {
          await fsp.rm(scratch, { force: true });
        }
      }

      const targetRel = [baseDir, relPath].filter(Boolean).join('/');
      const targetAbs = resolveInRoot(folder.path, targetRel);
      await fsp.mkdir(path.dirname(targetAbs), { recursive: true });
      const finalAbs = await uniquePath(path.dirname(targetAbs), path.basename(targetAbs));
      const part = `${finalAbs}.${crypto.randomBytes(4).toString('hex')}.part`;
      try {
        await pipeline(req.raw, fs.createWriteStream(part));
        // keep the original modification time (phones/desktop pass it) — it is the date fallback
        const mtime = Number(req.query.mtime);
        if (Number.isFinite(mtime) && mtime > 0 && mtime < 4102444800000) await fsp.utimes(part, new Date(), new Date(mtime));
        await fsp.rename(part, finalAbs);
      } catch (e) {
        await fsp.rm(part, { force: true });
        throw e;
      }
      const rel = relOf(folder.path, finalAbs);
      await ctx.scanner.upsertFile(folder.id, rel);
      return { created: [{ folderId: String(folder.id), dir: dirOfRel(rel), name: path.posix.basename(rel) }] };
    },
  );

  /** Peek into an archive that already lives in a library (temp session, library untouched). */
  app.post<{ Body: { id: string } }>('/api/zip/peek', HEAVY_LIMIT, async (req) => {
    const r = mediaRow(ctx, req.body.id);
    if (r.kind !== 'archive') throw new HttpError(400, 'Not an archive');
    const { abs } = mediaAbs(ctx, r);
    return { tempFolder: await zipToTemp(ctx, abs, r.name) };
  });

  /** Extract an archive that lives in a library next to itself. */
  app.post<{ Body: { id: string } }>('/api/zip/extract', HEAVY_LIMIT, async (req) => {
    requireWritable(ctx);
    const r = mediaRow(ctx, req.body.id);
    if (r.kind !== 'archive') throw new HttpError(400, 'Not an archive');
    const { abs, folder } = mediaAbs(ctx, r);
    const dest = await uniquePath(path.dirname(abs), r.name.replace(/\.zip$/i, '') || 'Archive');
    await extractZip(abs, dest);
    const destRel = relOf(folder.path, dest);
    await ctx.scanner.scanFolder(folder.id, destRel);
    return { folderId: String(folder.id), dir: destRel };
  });

  /** Desktop only: import files/folders by absolute path (drag & drop from the OS without uploading). */
  app.post<{ Body: { paths: string[]; folderId: string; dir: string; zip?: UploadZipMode } }>('/api/import-local', HEAVY_LIMIT, async (req) => {
    requireWritable(ctx);
    requireLocal(req);
    if (ctx.cfg.mode !== 'desktop') throw new HttpError(403, 'Only available in the desktop app');
    const { folder, abs: destDir } = folderPath(ctx, req.body.folderId, req.body.dir ?? '');
    await fsp.mkdir(destDir, { recursive: true });
    const result: UploadResult = { created: [] };
    for (const raw of Array.isArray(req.body.paths) ? req.body.paths : []) {
      const src = resolveHostPath(raw, ctx.cfg.hostBrowseRoots);
      const st = await fsp.stat(src).catch(() => null);
      if (!st) continue;
      const base = sanitizeName(path.basename(src));
      if (st.isFile() && extOf(base) === 'zip' && req.body.zip && req.body.zip !== 'keep') {
        if (req.body.zip === 'temp') {
          result.tempFolder = await zipToTemp(ctx, src, base);
        } else {
          const dest = await uniquePath(destDir, base.replace(/\.zip$/i, ''));
          await extractZip(src, dest);
          result.extractedDir = relOf(folder.path, dest);
          await ctx.scanner.scanFolder(folder.id, result.extractedDir);
        }
        continue;
      }
      const dest = await uniquePath(destDir, base);
      await fsp.cp(src, dest, { recursive: true, errorOnExist: true, force: false, preserveTimestamps: true });
      const rel = relOf(folder.path, dest);
      if (st.isDirectory()) await ctx.scanner.scanFolder(folder.id, rel);
      else await ctx.scanner.upsertFile(folder.id, rel);
      result.created.push({ folderId: String(folder.id), dir: dirOfRel(rel), name: path.posix.basename(rel) });
    }
    return result;
  });

  // ── file operations ──

  app.post<{ Body: { folderId: string; dir: string; name: string } }>('/api/fs/mkdir', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    const { folder, abs } = folderPath(ctx, req.body.folderId, req.body.dir ?? '');
    const dest = await uniquePath(abs, sanitizeName(req.body.name || 'New folder'));
    await fsp.mkdir(dest, { recursive: true });
    ctx.events.mediaChanged(String(folder.id), normalizeRel(req.body.dir));
    return { folderId: String(folder.id), dir: relOf(folder.path, dest) };
  });

  app.post<{ Body: FileRef & { name: string } }>('/api/fs/rename', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    const { folder, abs } = folderPath(ctx, req.body.folderId, req.body.rel);
    const relSrc = normalizeRel(req.body.rel);
    if (!relSrc) throw new HttpError(400, 'Cannot rename the folder root');
    const st = await fsp.stat(abs).catch(() => null);
    if (!st) throw new HttpError(404, 'Not found');
    const newName = sanitizeName(req.body.name);
    const dest = path.join(path.dirname(abs), newName);
    if (dest === abs) return { rel: relSrc };
    if (fs.existsSync(dest) && dest.toLowerCase() !== abs.toLowerCase()) throw new HttpError(409, 'A file with that name already exists');
    // keep favorites across renames
    const fav = st.isFile()
      ? ctx.db.get<{ favorite: number }>('SELECT favorite FROM media WHERE folder_id = ? AND dir = ? AND name = ?', folder.id, dirOfRel(relSrc), path.posix.basename(relSrc))?.favorite
      : 0;
    await fsp.rename(abs, dest);
    const relDest = relOf(folder.path, dest);
    if (st.isDirectory()) {
      ctx.scanner.removeDir(folder.id, relSrc);
      await ctx.scanner.scanFolder(folder.id, relDest);
    } else {
      ctx.scanner.removeFile(folder.id, relSrc);
      const id = await ctx.scanner.upsertFile(folder.id, relDest);
      if (id && fav) ctx.db.run('UPDATE media SET favorite = 1 WHERE id = ?', id);
    }
    return { rel: relDest };
  });

  /**
   * Move / copy. Linked phones: copy FROM a device folder into a PC folder and copy INTO a device
   * folder run as transfer jobs → response `{ moved: string[], job: Job }` (moved = local part done
   * synchronously). Moving from/to a device → 403.
   */
  app.post<{ Body: { refs: FileRef[]; to: { folderId: string; dir: string }; copy?: boolean } }>('/api/fs/move', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    const allRefs = req.body.refs ?? [];
    const target = ctx.lib.folderRow(req.body.to.folderId);
    const deviceRefs = allRefs.filter((r) => !!ctx.lib.folderRow(r.folderId).device_id);
    if (target.device_id || deviceRefs.length) {
      if (!req.body.copy) throw new HttpError(403, 'Files on a linked device can only be copied');
      if (target.device_id) {
        if (deviceRefs.length) throw new HttpError(403, 'Copying between devices is not supported');
        return { moved: [], job: await copyToDevice(ctx, allRefs, target, req.body.to.dir ?? '') };
      }
    }
    const { folder: toFolder, abs: toAbs } = folderPath(ctx, req.body.to.folderId, req.body.to.dir ?? '');
    await fsp.mkdir(toAbs, { recursive: true });
    const out: string[] = [];
    const job = deviceRefs.length ? copyFromDevice(ctx, deviceRefs, toFolder, req.body.to.dir ?? '') : null;
    for (const ref of allRefs) {
      if (deviceRefs.includes(ref)) continue;
      const { folder, abs } = folderPath(ctx, ref.folderId, ref.rel);
      const relSrc = normalizeRel(ref.rel);
      if (!relSrc) continue;
      if (toAbs === abs || toAbs.startsWith(abs + path.sep)) throw new HttpError(400, 'Cannot move a folder into itself');
      const st = await fsp.stat(abs).catch(() => null);
      if (!st) continue;
      if (!req.body.copy && path.dirname(abs) === toAbs) continue;
      const dest = await uniquePath(toAbs, path.basename(abs));
      if (req.body.copy) await fsp.cp(abs, dest, { recursive: true, errorOnExist: true, force: false, preserveTimestamps: true });
      else await moveFile(abs, dest);
      const relDest = relOf(toFolder.path, dest);
      if (!req.body.copy) {
        if (st.isDirectory()) ctx.scanner.removeDir(folder.id, relSrc);
        else ctx.scanner.removeFile(folder.id, relSrc);
      }
      if (st.isDirectory()) await ctx.scanner.scanFolder(toFolder.id, relDest);
      else await ctx.scanner.upsertFile(toFolder.id, relDest);
      out.push(relDest);
    }
    return job ? { moved: out, job } : { moved: out };
  });

  app.post<{ Body: { refs: FileRef[] } }>('/api/fs/delete', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    let n = 0;
    for (const ref of req.body.refs ?? []) {
      const { folder, abs } = folderPath(ctx, ref.folderId, ref.rel);
      const relSrc = normalizeRel(ref.rel);
      if (!relSrc) continue;
      const st = await fsp.stat(abs).catch(() => null);
      if (!st) continue;
      if (folder.temp) {
        await fsp.rm(abs, { recursive: true, force: true });
      } else if (ctx.cfg.hooks.trash) {
        await ctx.cfg.hooks.trash(abs);
      } else {
        const stamp = new Date().toISOString().replace(/[:.]/g, '-');
        const dest = path.join(folder.path, TRASH_DIR, stamp, ...relSrc.split('/'));
        await moveFile(abs, dest);
      }
      if (st.isDirectory()) ctx.scanner.removeDir(folder.id, relSrc);
      else ctx.scanner.removeFile(folder.id, relSrc);
      n++;
    }
    return { deleted: n };
  });

  // ── saving edited copies (photo editor, collage, video frame grabs) ──

  /**
   * POST /api/edits/save?sourceId=…&format=jpg|png|webp&quality=92   → copy next to source ("name_edit.jpg")
   * POST /api/edits/save?folderId=…&dir=…&name=Collage&format=jpg      → new file in a folder
   * Body: the rendered image (any format sharp can read; PNG recommended).
   */
  app.post<{ Querystring: { sourceId?: string; folderId?: string; dir?: string; name?: string; format?: string; quality?: string } }>(
    '/api/edits/save',
    { ...FS_LIMIT, bodyLimit: 1024 * 1024 * 1024 },
    async (req) => {
      requireWritable(ctx);
      const fmt = (['jpg', 'png', 'webp'].includes(req.query.format ?? '') ? req.query.format : 'jpg') as 'jpg' | 'png' | 'webp';
      const quality = Math.max(40, Math.min(100, Number(req.query.quality) || 92));
      const chunks: Buffer[] = [];
      for await (const c of req.raw) chunks.push(c as Buffer);
      const input = Buffer.concat(chunks);
      if (!input.length) throw new HttpError(400, 'Empty body');

      let folderRow;
      let destAbs: string;
      let exif: Record<string, Record<string, string>> | undefined;
      const suffix = getSettings(ctx).editSuffix;

      if (req.query.sourceId) {
        const src = mediaRow(ctx, req.query.sourceId);
        const { folder, abs } = mediaAbs(ctx, src);
        folderRow = folder;
        const base = src.name.slice(0, src.name.length - (src.ext ? src.ext.length + 1 : 0));
        destAbs = await uniqueEditPath(path.dirname(abs), base, suffix, fmt);
        exif = await exifFrom(abs);
      } else {
        const { folder, abs } = folderPath(ctx, req.query.folderId ?? '', req.query.dir ?? '');
        folderRow = folder;
        await fsp.mkdir(abs, { recursive: true });
        destAbs = await uniquePath(abs, `${sanitizeName(req.query.name || 'PeekAround')}.${fmt}`);
      }

      let img = sharp(input, { limitInputPixels: 1e9 });
      if (exif && fmt !== 'png') img = img.withExif(exif);
      if (fmt === 'jpg') img = img.flatten({ background: '#ffffff' }).jpeg({ quality, mozjpeg: true, chromaSubsampling: quality >= 95 ? '4:4:4' : '4:2:0' });
      else if (fmt === 'webp') img = img.webp({ quality });
      else img = img.png({ compressionLevel: 8 });
      const tmp = `${destAbs}.part`;
      await img.toFile(tmp);
      await fsp.rename(tmp, destAbs);
      const rel = relOf(folderRow.path, destAbs);
      const id = await ctx.scanner.upsertFile(folderRow.id, rel);
      return { item: { folderId: String(folderRow.id), dir: dirOfRel(rel), name: path.posix.basename(rel) }, id: id ? String(id) : null };
    },
  );

  // ── fonts ──
  app.get('/api/fonts', async () => ({ fonts: listFonts(ctx) }));

  app.put<{ Querystring: { name: string } }>('/api/fonts', { ...FS_LIMIT, bodyLimit: 64 * 1024 * 1024 }, async (req) => {
    const name = sanitizeName(req.query.name || 'font.ttf');
    const ext = extOf(name);
    if (!['ttf', 'otf', 'woff', 'woff2'].includes(ext)) throw new HttpError(400, 'Unsupported font format');
    const chunks: Buffer[] = [];
    for await (const c of req.raw) chunks.push(c as Buffer);
    const buf = Buffer.concat(chunks);
    const info = await parseFont(buf, name);
    await fsp.mkdir(ctx.cfg.fontDir, { recursive: true });
    const file = `${crypto.randomUUID()}.${ext}`;
    await fsp.writeFile(path.join(ctx.cfg.fontDir, file), buf);
    ctx.db.run(
      'INSERT INTO fonts (family, subfamily, weight, style, format, file, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
      info.family,
      info.subfamily,
      info.weight,
      info.style,
      formatOf(ext),
      file,
      Date.now(),
    );
    ctx.events.emit({ type: 'fonts' });
    return { fonts: listFonts(ctx) };
  });

  app.delete<{ Params: { id: string } }>('/api/fonts/:id', FS_LIMIT, async (req) => {
    const r = ctx.db.get<{ file: string }>('SELECT file FROM fonts WHERE id = ?', Number(req.params.id));
    if (r) {
      ctx.db.run('DELETE FROM fonts WHERE id = ?', Number(req.params.id));
      await fsp.rm(path.join(ctx.cfg.fontDir, r.file), { force: true });
      ctx.events.emit({ type: 'fonts' });
    }
    return { fonts: listFonts(ctx) };
  });

  app.get<{ Params: { id: string } }>('/api/fonts/:id/file', FS_LIMIT, async (req, reply) => {
    const r = ctx.db.get<{ file: string; format: string }>('SELECT file, format FROM fonts WHERE id = ?', Number(req.params.id));
    if (!r) throw new HttpError(404, 'Font not found');
    reply.header('Cache-Control', 'public, max-age=31536000, immutable');
    reply.header('Access-Control-Allow-Origin', '*');
    return reply.sendFile(r.file, ctx.cfg.fontDir, { cacheControl: false });
  });
};

function formatOf(ext: string) {
  return ext === 'ttf' ? 'truetype' : ext === 'otf' ? 'opentype' : ext;
}

function listFonts(ctx: Ctx) {
  return ctx.db.all<any>('SELECT * FROM fonts ORDER BY family COLLATE NOCASE, weight').map((r) => ({
    id: String(r.id),
    family: r.family,
    subfamily: r.subfamily,
    weight: r.weight,
    style: r.style,
    format: r.format,
    url: `/api/fonts/${r.id}/file`,
    bundled: false,
  }));
}

async function parseFont(buf: Buffer, fileName: string) {
  const fallback = fileName.replace(/\.[^.]+$/, '').replace(/[-_]+/g, ' ');
  try {
    const fontkit: any = await import('fontkit');
    const f = (fontkit.create ?? fontkit.default.create)(buf);
    const font = f.fonts ? f.fonts[0] : f;
    const sub: string = font.subfamilyName || 'Regular';
    const os2 = font['OS/2'];
    return {
      family: String(font.familyName || fallback).slice(0, 80),
      subfamily: sub,
      weight: Number(os2?.usWeightClass) || (/bold/i.test(sub) ? 700 : 400),
      style: /italic|oblique/i.test(sub) || font.italicAngle ? 'italic' : 'normal',
    };
  } catch {
    throw new HttpError(400, 'Invalid or unsupported font file');
  }
}

/** Carry the important EXIF tags (capture date, camera) over to the edited copy. */
async function exifFrom(abs: string): Promise<Record<string, Record<string, string>> | undefined> {
  try {
    const ex = await exifr.parse(abs, { pick: ['DateTimeOriginal', 'CreateDate', 'Make', 'Model', 'LensModel'], reviveValues: false });
    if (!ex) return undefined;
    const ifd0: Record<string, string> = { Software: 'PeekAround' };
    const ifd2: Record<string, string> = {};
    if (ex.Make) ifd0.Make = String(ex.Make);
    if (ex.Model) ifd0.Model = String(ex.Model);
    if (ex.DateTimeOriginal) ifd2.DateTimeOriginal = String(ex.DateTimeOriginal);
    if (ex.CreateDate) ifd2.DateTimeDigitized = String(ex.CreateDate);
    if (ex.LensModel) ifd2.LensModel = String(ex.LensModel);
    return { IFD0: ifd0, IFD2: ifd2 };
  } catch {
    return undefined;
  }
}
