import fs from 'node:fs';
import { FS_LIMIT, HEAVY_LIMIT } from '../security.js';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import type { HostDirEntry, LibraryOptions, LibraryStats, ServerInfo, Settings } from '@peekaround/shared';
import { APP_NAME, API_VERSION, isValidTimeZone } from '@peekaround/shared';
import { lanUrls } from '../config.js';
import { getSettings, saveSettings, type Ctx } from '../context.js';
import { HttpError, isInside, resolveInRoot, sanitizeName, uniquePath, resolveHostPath } from '../paths.js';
import { moveFile, requireLocal, requireLocalFolder, requireWritable, type Plugin } from './util.js';
import { canSeePairing, viewerOf } from '../devices/access.js';
import { tileService } from './tiles.js';
import { pickRuntimeSettings } from '../runtime-settings.js';

export function serverInfo(ctx: Ctx): ServerInfo {
  const desktop = ctx.cfg.mode === 'desktop';
  const ro = ctx.cfg.readonly;
  return {
    name: APP_NAME,
    version: ctx.cfg.version,
    apiVersion: API_VERSION,
    mode: ctx.cfg.mode,
    hostname: os.hostname(),
    scanning: ctx.scanner.busy,
    caps: {
      upload: !ro,
      fileOps: !ro,
      nativeFolders: desktop,
      browseHost: !desktop && !ro,
      videoJobs: ctx.ffmpeg.available && !ro,
      slideshowExport: ctx.ffmpeg.available && !ro,
      zipDownload: true,
      trash: ro ? 'none' : ctx.cfg.hooks.trash ? 'os' : 'folder',
      readonly: ro,
      lanShare: desktop,
    },
    lan: {
      enabled: ctx.cfg.mode === 'server' || ctx.lan.enabled,
      urls: ctx.cfg.mode === 'server' || ctx.lan.enabled ? lanUrls(ctx.cfg.mode === 'server' ? ctx.cfg.port : ctx.lan.port) : [],
    },
  };
}

export const coreRoutes: Plugin = (app, ctx) => {
  app.get('/api/info', async () => serverInfo(ctx));

  app.get('/api/settings', FS_LIMIT, async () => getSettings(ctx));
  app.patch<{ Body: Partial<Settings> }>('/api/settings', FS_LIMIT, async (req) => {
    const b = req.body ?? {};
    const patch: Partial<Settings> = {};
    if (b.zipAction && ['ask', 'extract', 'temp'].includes(b.zipAction)) patch.zipAction = b.zipAction;
    if (typeof b.editSuffix === 'string') patch.editSuffix = b.editSuffix.replace(/[\\/:*?"<>|]/g, '').slice(0, 32) || '_edit';
    if (typeof b.showHidden === 'boolean') patch.showHidden = b.showHidden;
    if (typeof b.tempSessionHours === 'number') patch.tempSessionHours = Math.max(1, Math.min(24 * 30, b.tempSessionHours));
    if (b.homeTimeZone !== undefined) {
      if (!isValidTimeZone(b.homeTimeZone)) throw new HttpError(400, 'Unknown time zone');
      patch.homeTimeZone = b.homeTimeZone;
    }
    if (b.mapTiles && ['carto', 'osm', 'topo', 'satellite', 'off'].includes(b.mapTiles)) patch.mapTiles = b.mapTiles;
    if (typeof b.tileCacheMb === 'number' && Number.isFinite(b.tileCacheMb)) patch.tileCacheMb = Math.round(Math.max(32, Math.min(16384, b.tileCacheMb)));
    // remote (MEGA) stream cache cap, MB (0 = keep nothing)
    if (typeof b.remoteCacheMb === 'number' && Number.isFinite(b.remoteCacheMb)) patch.remoteCacheMb = Math.round(Math.max(0, Math.min(1024 * 1024, b.remoteCacheMb)));
    // thumbnail quality, indexing intensity, ffmpeg hwaccel (runtime-settings.ts)
    pickRuntimeSettings(b, patch);
    const s = saveSettings(ctx, patch);
    if (patch.remoteCacheMb !== undefined) await ctx.remote.cache.setCap(patch.remoteCacheMb * 1024 * 1024);
    if (patch.showHidden !== undefined) void ctx.scanner.scanAll();
    if (patch.tileCacheMb !== undefined) void tileService(ctx).trim();
    return s;
  });

  // ── libraries ──
  // devices: linked phones with their folders (only for viewers allowed to see them, never the asking phone itself)
  app.get('/api/libraries', FS_LIMIT, async (req) => {
    const v = viewerOf(ctx, req);
    return { libraries: ctx.lib.listLibraries(), temp: ctx.lib.listTemp(), devices: v.devices ? ctx.devices.list(v.self) : [] };
  });

  app.post<{ Body: { name: string; color?: string; icon?: string; folders?: string[] } }>('/api/libraries', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    const b = req.body;
    if (b.folders?.length) requireLocal(req);
    for (const f of b.folders ?? []) checkHostPath(ctx, f);
    const lib = ctx.lib.createLibrary(b);
    ctx.watcher.sync();
    for (const f of lib.folders) void ctx.scanner.scanFolder(Number(f.id));
    return lib;
  });

  app.patch<{ Params: { id: string }; Body: { name?: string; color?: string; icon?: string; sort?: number; coverId?: string | null; options?: Partial<LibraryOptions> } }>(
    '/api/libraries/:id', FS_LIMIT, async (req) => {
      requireWritable(ctx);
      const { rescan, ...lib } = ctx.lib.updateLibrary(req.params.id, req.body ?? {});
      // exclusions / minimum size / hidden folders changed → re-walk (excluded rows drop out, re-included files come back)
      if (rescan) {
        ctx.scanner.invalidateRules();
        for (const f of lib.folders) void ctx.scanner.scanFolder(Number(f.id));
      }
      return lib;
    },
  );

  // new library order (sidebar drag): { ids } first → last
  app.post<{ Body: { ids: string[] } }>('/api/libraries/order', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    ctx.lib.reorderLibraries(Array.isArray(req.body?.ids) ? req.body.ids.map(String) : []);
    return { libraries: ctx.lib.listLibraries() };
  });

  // header numbers of a library page
  app.get<{ Params: { id: string } }>('/api/libraries/:id/stats', FS_LIMIT, async (req): Promise<LibraryStats> =>
    ctx.lib.stats(req.params.id, (ids) => ctx.scanner.busyFor(ids)),
  );

  app.post<{ Params: { id: string } }>('/api/libraries/:id/rescan', HEAVY_LIMIT, async (req) => {
    const lib = ctx.lib.library(req.params.id);
    for (const f of lib.folders) void ctx.scanner.scanFolder(Number(f.id));
    return { ok: true };
  });

  app.delete<{ Params: { id: string } }>('/api/libraries/:id', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    ctx.lib.deleteLibrary(req.params.id);
    ctx.watcher.sync();
    return { ok: true };
  });

  app.post<{ Params: { id: string }; Body: { path: string; name?: string } }>('/api/libraries/:id/folders', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    requireLocal(req);
    checkHostPath(ctx, req.body.path);
    const row = ctx.lib.addFolder(req.params.id, req.body.path, req.body.name);
    ctx.watcher.sync();
    void ctx.scanner.scanFolder(row.id);
    return ctx.lib.folderRef(row);
  });

  app.patch<{ Params: { id: string }; Body: { name: string } }>('/api/folders/:id', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    const f = ctx.lib.folderRow(req.params.id);
    // a remote folder's name is only a label in our database
    if (!f.remote) requireLocalFolder(f, 'Device folders are named on the phone');
    ctx.lib.renameFolder(req.params.id, req.body.name);
    return { ok: true };
  });

  app.delete<{ Params: { id: string } }>('/api/folders/:id', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    const row = ctx.lib.folderRow(req.params.id);
    // remote (MEGA) folders: "remove link" — only our rows go, nothing on MEGA is touched
    if (!row.remote) requireLocalFolder(row, 'Stop sharing the folder on the phone, or forget the device');
    if (row.temp) await ctx.lib.deleteTemp(row, ctx.cfg.tempDir);
    else ctx.lib.removeFolder(req.params.id);
    if (row.remote) ctx.remote.forget(row.id);
    ctx.events.emit({ type: 'folders' });
    ctx.watcher.sync();
    return { ok: true };
  });

  app.post<{ Params: { id: string } }>('/api/folders/:id/rescan', HEAVY_LIMIT, async (req) => {
    void ctx.scanner.scanFolder(Number(req.params.id));
    return { ok: true };
  });

  // ── temp sessions ──
  app.post<{ Params: { id: string }; Body: { folderId: string; dir: string } }>('/api/temp/:id/keep', HEAVY_LIMIT, async (req) => {
    requireWritable(ctx);
    const temp = ctx.lib.folderRow(req.params.id);
    if (!temp.temp) throw new HttpError(400, 'Not a temporary session');
    const target = ctx.lib.folderRow(req.body.folderId);
    requireLocalFolder(target);
    const destParent = resolveInRoot(target.path, req.body.dir ?? '');
    const dest = await uniquePath(destParent, temp.name || 'Archive');
    await moveFile(temp.path, dest);
    await ctx.lib.deleteTemp(temp, ctx.cfg.tempDir);
    const relDir = path.relative(target.path, dest).split(path.sep).join('/');
    await ctx.scanner.scanFolder(target.id);
    return { folderId: String(target.id), dir: relDir };
  });

  app.post<{ Params: { id: string }; Body: { hours: number } }>('/api/temp/:id/extend', FS_LIMIT, async (req) => {
    const temp = ctx.lib.folderRow(req.params.id);
    if (!temp.temp) throw new HttpError(400, 'Not a temporary session');
    const hours = Math.max(1, Math.min(24 * 30, Number(req.body?.hours) || 0));
    ctx.db.run('UPDATE folders SET expires_at = ? WHERE id = ?', Date.now() + hours * 3600_000, temp.id);
    ctx.events.emit({ type: 'folders' });
    return { ok: true };
  });

  // ── host directory browser (for adding folders in server mode) ──
  app.get<{ Querystring: { path?: string } }>('/api/host/dirs', FS_LIMIT, async (req) => {
    requireLocal(req);
    const roots = hostRoots(ctx);
    if (!req.query.path) {
      return { path: null, parent: null, dirs: roots.map<HostDirEntry>((r) => ({ name: r, path: r })) };
    }
    const p = checkHostPath(ctx, req.query.path);
    const entries = await fsp.readdir(p, { withFileTypes: true }).catch(() => {
      throw new HttpError(400, 'Cannot read directory');
    });
    const dirs = entries
      .filter((e) => e.isDirectory() && !e.name.startsWith('.'))
      .map<HostDirEntry>((e) => ({ name: e.name, path: path.join(p, e.name) }))
      .sort((a, b) => a.name.localeCompare(b.name));
    const parent = path.dirname(p);
    const parentAllowed = parent !== p && (!ctx.cfg.hostBrowseRoots.length || ctx.cfg.hostBrowseRoots.some((r) => isInside(r, parent)));
    return { path: p, parent: parentAllowed ? parent : null, dirs };
  });

  app.post<{ Body: { path: string; name: string } }>('/api/host/mkdir', FS_LIMIT, async (req) => {
    requireWritable(ctx);
    requireLocal(req);
    const base = checkHostPath(ctx, req.body.path);
    const p = path.join(base, sanitizeName(req.body.name));
    if (!isInside(base, p)) throw new HttpError(400, 'Invalid name');
    await fsp.mkdir(p, { recursive: true });
    return { path: p };
  });

  // ── realtime ──
  app.get('/api/events', { websocket: true }, (socket, req) => {
    socket.send(JSON.stringify({ type: 'hello', info: serverInfo(ctx) }));
    // pairing requests only reach clients that may approve them (desktop: the local window)
    const pairing = canSeePairing(ctx, req);
    const off = ctx.events.on((e) => {
      if (e.type === 'pair' && !pairing) return;
      if (socket.readyState === 1) socket.send(JSON.stringify(e));
    });
    const ping = setInterval(() => {
      if (socket.readyState === 1) socket.ping();
    }, 25000);
    socket.on('close', () => {
      off();
      clearInterval(ping);
    });
  });

  // ── cache ──
  // bytes = thumbnails; remoteBytes = streamed MEGA originals (LRU, Settings.remoteCacheMb)
  app.get('/api/cache', FS_LIMIT, async () => ({ bytes: await ctx.thumbs.sizeBytes(), remoteBytes: await ctx.remote.cache.sizeBytes() }));
  app.delete('/api/cache', FS_LIMIT, async (req) => {
    requireLocal(req);
    await ctx.thumbs.clear();
    await ctx.remote.cache.clear();
    ctx.db.run('UPDATE media SET thumb_state = 0');
    ctx.scanner.kick();
    ctx.remote.kick();
    return { ok: true };
  });
};

function hostRoots(ctx: Ctx): string[] {
  if (ctx.cfg.hostBrowseRoots.length) return ctx.cfg.hostBrowseRoots.filter((r) => fs.existsSync(r));
  if (process.platform === 'win32') {
    const out: string[] = [];
    for (let c = 67; c <= 90; c++) {
      const d = `${String.fromCharCode(c)}:\\`;
      if (fs.existsSync(d)) out.push(d);
    }
    return [os.homedir(), ...out];
  }
  return [os.homedir(), '/'];
}

function checkHostPath(ctx: Ctx, p: string): string {
  if (!p) throw new HttpError(400, 'Path required');
  return resolveHostPath(p, ctx.cfg.hostBrowseRoots);
}
