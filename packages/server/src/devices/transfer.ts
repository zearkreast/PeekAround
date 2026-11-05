import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { kindOf, type FileRef, type Job } from '@peekaround/shared';
import type { Ctx } from '../context.js';
import type { FolderRow } from '../library.js';
import type { MediaRow } from '../media/mapper.js';
import { HttpError, normalizeRel, resolveInRoot, sanitizeName, toPosix, uniquePath } from '../paths.js';
import type { TunnelReply } from './hub.js';
import { relOfRow } from './media.js';

/** Counts bytes flowing through (job progress) without buffering. */
export function counter(onBytes: (n: number) => void): Transform {
  return new Transform({
    transform(chunk: Buffer, _enc, cb) {
      onBytes(chunk.length);
      cb(null, chunk);
    },
  });
}

interface FromItem {
  deviceId: string;
  key: string;
  row: MediaRow;
  /** destination path relative to the target dir ('/' separated) */
  dest: string;
}

/**
 * Copy device media (files or whole directories) into a PC folder as a transfer job:
 * `file` op → `<name>.<rand>.part` → unique final name → mtime from X-Peek-Mtime → index.
 */
export function copyFromDevice(ctx: Ctx, refs: FileRef[], to: FolderRow, toDir: string): Job {
  const dir = normalizeRel(toDir);
  const toAbs = resolveInRoot(to.path, dir);
  // expand refs (a file, or a directory → every media row below it, tree preserved)
  const groups: { top: string; file: boolean; items: FromItem[] }[] = [];
  for (const ref of refs) {
    const folder = ctx.lib.folderRow(ref.folderId);
    const { deviceId, key } = ctx.devices.folderOf(folder);
    const rel = normalizeRel(ref.rel);
    const name = path.posix.basename(rel);
    const parent = path.posix.dirname(rel) === '.' ? '' : path.posix.dirname(rel);
    const file = rel ? ctx.db.get<MediaRow>('SELECT * FROM media WHERE folder_id = ? AND dir = ? AND name = ?', folder.id, parent, name) : undefined;
    if (file) {
      groups.push({ top: name, file: true, items: [{ deviceId, key, row: file, dest: name }] });
      continue;
    }
    const rows = ctx.db.all<MediaRow>(
      `SELECT * FROM media WHERE folder_id = ?${rel ? ' AND (dir = ? OR substr(dir, 1, ?) = ?)' : ''} ORDER BY dir, name`,
      folder.id,
      ...(rel ? [rel, rel.length + 1, `${rel}/`] : []),
    );
    const top = rel ? name : folder.name;
    groups.push({
      top: sanitizeName(top),
      file: false,
      items: rows.map((row) => ({ deviceId, key, row, dest: relOfRow(row).slice(rel ? rel.length + 1 : 0) })),
    });
  }
  const items = groups.flatMap((g) => g.items);
  if (!items.length) throw new HttpError(400, 'Nothing to copy');
  for (const d of new Set(items.map((i) => i.deviceId))) if (!ctx.devices.isOnline(d)) throw new HttpError(503, 'device offline');
  const totalBytes = items.reduce((a, i) => a + (i.row.size || 0), 0);
  const title = items.length === 1 ? items[0].row.name : `${items.length} files`;

  return ctx.jobs.enqueue('transfer', title, async (jc) => {
    let done = 0;
    let bytes = 0;
    const failed: string[] = [];
    let first: string | null = null;
    jc.transfer({ bytes: { done: 0, total: totalBytes }, files: { done: 0, total: items.length } });
    await fsp.mkdir(toAbs, { recursive: true });
    for (const g of groups) {
      // a copied directory lands under a unique name next to existing ones
      const base = g.file ? toAbs : await uniquePath(toAbs, g.top);
      for (const it of g.items) {
        if (jc.signal.aborted) throw new Error('canceled');
        const targetDir = path.join(base, ...path.posix.dirname(it.dest).split('/').filter((s) => s && s !== '.'));
        const fileBytesBefore = bytes;
        try {
          const final = await fetchDeviceFile(ctx, it, targetDir, jc.signal, (n) => {
            bytes += n;
            jc.transfer({ bytes: { done: bytes, total: totalBytes }, files: { done, total: items.length } });
          });
          const rel = toPosix(path.relative(to.path, final));
          await ctx.scanner.upsertFile(to.id, rel);
          first ??= toPosix(path.relative(toAbs, final)).split('/')[0];
        } catch (e) {
          if (jc.signal.aborted) throw e;
          if (e instanceof HttpError && e.statusCode === 503) throw e;
          failed.push(it.row.name);
          bytes = fileBytesBefore + (it.row.size || 0);
        }
        done++;
        jc.transfer({ bytes: { done: bytes, total: totalBytes }, files: { done, total: items.length } });
      }
    }
    if (failed.length) throw new Error(`Copied ${done - failed.length} of ${done} files; failed: ${failed.slice(0, 5).join(', ')}${failed.length > 5 ? '…' : ''}`);
    return { folderId: String(to.id), dir, name: first ?? '' };
  });
}

async function fetchDeviceFile(ctx: Ctx, it: FromItem, targetDir: string, signal: AbortSignal, onBytes: (n: number) => void): Promise<string> {
  await fsp.mkdir(targetDir, { recursive: true });
  const name = sanitizeName(it.row.name);
  const part = path.join(targetDir, `${name}.${crypto.randomBytes(4).toString('hex')}.part`);
  const tr: TunnelReply = await ctx.devices.request(it.deviceId, 'file', { key: it.key, p: relOfRow(it.row) }, { prio: 1, signal });
  if (tr.status !== 200) {
    tr.body.destroy();
    throw new HttpError(502, `The device answered ${tr.status}`);
  }
  try {
    await pipeline(tr.body, counter(onBytes), fs.createWriteStream(part), { signal });
    const mt = Number(tr.headers['x-peek-mtime']) || it.row.mtime;
    if (mt > 0 && mt < 4102444800000) await fsp.utimes(part, new Date(), new Date(mt));
    const final = await uniquePath(targetDir, name);
    await fsp.rename(part, final);
    return final;
  } catch (e) {
    await fsp.rm(part, { force: true });
    throw e;
  }
}

interface ToItem {
  abs: string;
  /** target dir inside the device folder */
  dir: string;
  name: string;
  size: number;
  mtime: number;
}

/**
 * Copy PC files (or directories) into a device folder: per file a one-time pull URL + `receive`
 * op; the phone pulls, writes `.part`, renames to a unique name, keeps mtime and answers `res`.
 * Afterwards the touched directories are re-listed.
 */
export async function copyToDevice(ctx: Ctx, refs: FileRef[], deviceFolder: FolderRow, toDir: string): Promise<Job> {
  const { deviceId, key } = ctx.devices.folderOf(deviceFolder);
  const caps = ctx.devices.caps(deviceId);
  if (!caps) throw new HttpError(503, 'device offline');
  if (!caps.receive) throw new HttpError(403, 'This device does not accept files');
  const dir = normalizeRel(toDir);
  const items: ToItem[] = [];
  const walk = async (abs: string, targetDir: string) => {
    for (const e of await fsp.readdir(abs, { withFileTypes: true })) {
      if (ctx.scanner.skipName(e.name)) continue;
      const p = path.join(abs, e.name);
      if (e.isDirectory()) await walk(p, joinDir(targetDir, e.name));
      else if (e.isFile() && kindOf(e.name)) {
        const st = await fsp.stat(p);
        items.push({ abs: p, dir: targetDir, name: e.name, size: st.size, mtime: Math.round(st.mtimeMs) });
      }
    }
  };
  for (const ref of refs) {
    const folder = ctx.lib.folderRow(ref.folderId);
    if (folder.device_id) throw new HttpError(403, 'Copying between devices is not supported');
    const abs = resolveInRoot(folder.path, ref.rel);
    const st = await fsp.stat(abs).catch(() => null);
    if (!st) continue;
    if (st.isDirectory()) await walk(abs, joinDir(dir, path.basename(abs)));
    else if (st.isFile()) items.push({ abs, dir, name: path.basename(abs), size: st.size, mtime: Math.round(st.mtimeMs) });
  }
  if (!items.length) throw new HttpError(400, 'Nothing to copy');
  const totalBytes = items.reduce((a, i) => a + i.size, 0);
  const title = items.length === 1 ? items[0].name : `${items.length} files`;

  return ctx.jobs.enqueue('transfer', title, async (jc) => {
    let done = 0;
    let bytes = 0;
    let first: string | null = null;
    const touched = new Set<string>();
    // cancel also stops a pull the phone is still reading
    let active: import('node:stream').Readable | null = null;
    jc.signal.addEventListener('abort', () => active?.destroy(), { once: true });
    jc.transfer({ bytes: { done: 0, total: totalBytes }, files: { done: 0, total: items.length } });
    try {
      for (const it of items) {
        if (jc.signal.aborted) throw new Error('canceled');
        const before = bytes;
        let token = '';
        token = ctx.devices.createPull(deviceId, it.abs, it.size, it.mtime, {
          onStart: (stream) => {
            active = stream;
            const rid = ctx.devices.ridOfPull(token);
            if (rid) ctx.devices.holdReceive(rid);
          },
          onBytes: (n) => {
            bytes += n;
            jc.transfer({ bytes: { done: bytes, total: totalBytes }, files: { done, total: items.length } });
          },
          onEnd: () => {
            const rid = ctx.devices.ridOfPull(token);
            if (rid) ctx.devices.awaitReceiveResult(rid);
          },
        });
        try {
          const res = (await ctx.devices.request(
            deviceId,
            'receive',
            { key, dir: it.dir, name: it.name, size: it.size, mtime: it.mtime, url: `/api/devices/pull/${token}` },
            { prio: 1, signal: jc.signal },
          )) as { p?: string };
          first ??= typeof res?.p === 'string' ? res.p : joinDir(it.dir, it.name);
        } finally {
          ctx.devices.revokePull(token);
          active = null;
        }
        bytes = before + it.size;
        touched.add(it.dir);
        done++;
        jc.transfer({ bytes: { done: bytes, total: totalBytes }, files: { done, total: items.length } });
      }
    } finally {
      // refresh what landed (the phone also reports `changed`; relist is de-duplicated)
      for (const d of touched) void ctx.devices.relist(deviceFolder.id, d).catch(() => {});
    }
    const p = first ?? '';
    return { folderId: String(deviceFolder.id), dir: path.posix.dirname(p) === '.' ? '' : path.posix.dirname(p), name: path.posix.basename(p) };
  });
}

function joinDir(a: string, b: string) {
  return a ? `${a}/${b}` : b;
}

/** GET /api/devices/pull/:token body: the PC file, streamed (Content-Length, Last-Modified, X-Peek-Mtime). */
export async function openPull(ctx: Ctx, token: string, deviceId: string) {
  const p = ctx.devices.takePull(token, deviceId);
  const st = await fsp.stat(p.abs).catch(() => null);
  if (!st?.isFile()) throw new HttpError(404, 'File no longer exists');
  const stream = fs.createReadStream(p.abs);
  const counted = stream.pipe(counter((n) => p.onBytes?.(n)));
  p.onStart?.(counted);
  stream.on('error', (e) => counted.destroy(e));
  counted.on('end', () => p.onEnd?.());
  counted.on('close', () => stream.destroy());
  return { stream: counted, size: st.size, mtime: p.mtime || Math.round(st.mtimeMs) };
}
