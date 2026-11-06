import type {
  BrowseSkeleton,
  CalendarYear,
  ExploreResult,
  GeoPoints,
  BrowseQuery,
  BrowseResult,
  Crumb,
  FileRef,
  FolderRef,
  FontInfo,
  Job,
  Library,
  MediaDetails,
  MediaItem,
  ServerEvent,
  ServerInfo,
  Settings,
  SlideshowExportSpec,
  SubFolder,
  TimelineBucket,
  UploadResult,
  UploadZipMode,
  VideoEditSpec,
} from '@peekaround/shared';
import { API_VERSION, APP_NAME, extOf } from '@peekaround/shared';
import { ApiError, type Backend, type ConnectionState, type SaveEditOptions, type UploadOptions } from '../types';
import { loadLocal, saveLocal, type LocalConfig, type LocalFolder } from './config';
import { readFontInfo } from './fontname';
import { PeekLocal, writeBlob, type NativeFile } from './plugin';
import { buildSkeleton } from './skeleton';
import { buildExplore, calendarFor, filterExplore, geoPointsFor } from './explore';
import { ensureIndexed } from './indexBridge';

const COLORS = ['#8b5cf6', '#ec4899', '#f97316', '#10b981', '#06b6d4', '#3b82f6'];
const ICONS = ['camera', 'images', 'film', 'heart', 'star', 'plane'];

/** path ↔ media id (URL-safe base64 of the UTF-8 path) */
export function encId(path: string): string {
  const bytes = new TextEncoder().encode(path);
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function decId(id: string): string {
  const b = atob(id.replace(/-/g, '+').replace(/_/g, '/'));
  return new TextDecoder().decode(Uint8Array.from(b, (c) => c.charCodeAt(0)));
}

const join = (...p: string[]) => p.filter(Boolean).join('/').replace(/\/+/g, '/');
const dirname = (p: string) => p.slice(0, Math.max(0, p.lastIndexOf('/'))) || '/';
const basename = (p: string) => p.slice(p.lastIndexOf('/') + 1);
const rid = () => Math.random().toString(36).slice(2, 10) + Date.now().toString(36);

/**
 * The phone's own storage as a PeekAround backend.
 * Media bytes are served by the in-WebView media server (http://localhost/_peek/…).
 */
export class AndroidLocalBackend implements Backend {
  readonly id = 'android-local';
  readonly kind = 'android-local' as const;
  readonly label: string;
  private cfg: LocalConfig;
  private listeners = new Set<(e: ServerEvent) => void>();
  private cache = new Map<string, { at: number; data: Awaited<ReturnType<typeof PeekLocal.list>> }>();
  private jobsById = new Map<string, Job>();
  private dirs!: { cacheDir: string; filesDir: string };
  private ready: Promise<void>;
  private handles: { remove(): Promise<void> }[] = [];
  /** media id → MediaStore _ID (fast native thumbnails via /_peek/thumb?id=) */
  private msIds = new Map<string, number>();
  private nativeSig = '';

  constructor(label = 'Telefon') {
    this.label = label;
    this.cfg = loadLocal();
    this.ready = this.init();
  }

  private async init() {
    const r = await PeekLocal.roots();
    this.dirs = { cacheDir: r.cacheDir, filesDir: r.filesDir };
    this.syncNative();
    this.handles.push(
      await PeekLocal.addListener('changed', () => {
        this.cache.clear();
        this.emit({ type: 'media', folderId: '*', dirs: ['*'] });
      }),
    );
    this.handles.push(
      await PeekLocal.addListener('job', (e) => {
        const j: Job = {
          id: e.id,
          type: e.type,
          title: e.title,
          status: e.status,
          progress: e.progress,
          error: e.error,
          result: e.resultPath ? this.refFor(e.resultPath) : null,
          createdAt: this.jobsById.get(e.id)?.createdAt ?? Date.now(),
        };
        this.jobsById.set(j.id, j);
        if (e.status === 'done') this.cache.clear();
        this.emit({ type: 'job', job: j });
      }),
    );
    const now = Date.now();
    const expired = this.cfg.temp.filter((t) => t.expiresAt < now);
    if (expired.length) {
      await PeekLocal.remove({ paths: expired.map((t) => dirname(t.path)), permanent: true }).catch(() => {});
      this.cfg.temp = this.cfg.temp.filter((t) => t.expiresAt >= now);
      this.save();
    }
  }

  /** Create default libraries from the usual Android folders. */
  async setupDefaults() {
    await this.ready;
    if (this.cfg.libraries.length) return;
    const { roots } = await PeekLocal.roots();
    const want = ['Camera', 'Pictures', 'Movies', 'Download', 'WhatsApp'];
    for (const name of want) {
      const r = roots.find((x) => x.name === name);
      if (r) await this.createLibrary({ name: name === 'Camera' ? 'Kamera' : name, folders: [r.path] });
    }
    if (!this.cfg.libraries.length) {
      const dcim = roots.find((x) => x.name === 'DCIM') ?? roots[0];
      if (dcim) await this.createLibrary({ name: dcim.name, folders: [dcim.path] });
    }
  }

  private save() {
    saveLocal(this.cfg);
    this.syncNative();
  }

  /**
   * Keep the native side in step with the libraries: /_peek/* only serves library folders + temp
   * sessions (+ the app's cache/files dirs, always allowed natively), and the metadata indexer
   * (GPS, camera, offset) walks the library folders. Called after load and on every change.
   */
  private syncNative() {
    if (!this.dirs) return;
    const lib = this.cfg.libraries.flatMap((l) => l.folders.map((f) => f.path));
    const temp = this.cfg.temp.map((t) => t.path);
    const allowed = [...new Set([...lib, ...temp, this.dirs.cacheDir, this.dirs.filesDir].filter(Boolean))].sort();
    const sig = allowed.join('\n');
    if (sig === this.nativeSig) return;
    this.nativeSig = sig;
    void PeekLocal.setAllowedRoots({ paths: allowed }).catch(() => {});
    const roots = [...new Set(lib)].sort();
    if (roots.length) void ensureIndexed(roots, null).catch(() => {});
  }

  private emit(e: ServerEvent) {
    for (const l of this.listeners) l(e);
  }

  private allFolders(): (LocalFolder & { libraryId: string | null; temp: boolean; expiresAt: number | null; sourceName: string | null })[] {
    return [
      ...this.cfg.libraries.flatMap((l) => l.folders.map((f) => ({ ...f, libraryId: l.id, temp: false, expiresAt: null, sourceName: null }))),
      ...this.cfg.temp.map((t) => ({ ...t, libraryId: null, temp: true, expiresAt: t.expiresAt, sourceName: t.sourceName })),
    ];
  }

  private folder(id: string) {
    const f = this.allFolders().find((x) => x.id === id);
    if (!f) throw new ApiError(404, 'Folder not found');
    return f;
  }

  private folderRef(f: ReturnType<AndroidLocalBackend['folder']>): FolderRef {
    return { id: f.id, path: f.path, name: f.name, libraryId: f.libraryId, temp: f.temp, expiresAt: f.expiresAt, sourceName: f.sourceName, mediaCount: this.cfg.counts[f.id] ?? 0, device: null };
  }

  /** absolute path → { folderId, dir, name } within the libraries */
  private refFor(abs: string): { folderId: string; dir: string; name: string } | null {
    const f = this.allFolders()
      .filter((x) => abs.startsWith(x.path + '/'))
      .sort((a, b) => b.path.length - a.path.length)[0];
    if (!f) return null;
    const rel = abs.slice(f.path.length + 1);
    return { folderId: f.id, dir: rel.includes('/') ? rel.slice(0, rel.lastIndexOf('/')) : '', name: basename(abs) };
  }

  private toItem(n: NativeFile, f: { id: string; path: string }): MediaItem {
    const rel = n.path.slice(f.path.length + 1);
    const ext = extOf(n.name);
    const id = encId(n.path);
    if (n.id != null) this.msIds.set(id, n.id);
    return {
      id,
      folderId: f.id,
      dir: rel.includes('/') ? rel.slice(0, rel.lastIndexOf('/')) : '',
      name: n.name,
      ext,
      kind: n.kind,
      size: n.size,
      mtime: n.mtime,
      width: n.width,
      height: n.height,
      duration: n.duration,
      takenAt: n.takenAt || n.mtime,
      localAt: (n.takenAt || n.mtime) - new Date(n.takenAt || n.mtime).getTimezoneOffset() * 60000,
      flags: 0,
      thumbhash: null,
      favorite: this.cfg.favorites.includes(n.path),
      playback: ext === 'heic' || ext === 'heif' ? 'convert' : 'direct',
    };
  }

  private async list(path: string, recursive: boolean) {
    await this.ready;
    const key = `${path}|${recursive}`;
    const hit = this.cache.get(key);
    if (hit && Date.now() - hit.at < 60_000) return hit.data;
    const data = await PeekLocal.list({ path, recursive, showHidden: this.cfg.settings.showHidden });
    this.cache.set(key, { at: Date.now(), data });
    return data;
  }

  // ───────────────────────── info & settings ─────────────────────────

  async info(): Promise<ServerInfo> {
    return {
      name: APP_NAME,
      version: '1.3.0',
      apiVersion: API_VERSION,
      mode: 'android-local',
      hostname: this.label,
      scanning: false,
      caps: {
        upload: true,
        fileOps: true,
        nativeFolders: false,
        browseHost: true,
        videoJobs: true,
        slideshowExport: true,
        zipDownload: false,
        trash: 'folder',
        readonly: false,
        lanShare: false,
      },
      lan: { enabled: false, urls: [] },
    };
  }

  async settings() {
    return this.cfg.settings;
  }

  async updateSettings(patch: Partial<Settings>) {
    this.cfg.settings = { ...this.cfg.settings, ...patch };
    this.save();
    this.cache.clear();
    return this.cfg.settings;
  }

  // ───────────────────────── libraries ─────────────────────────

  async libraries() {
    await this.ready;
    const libraries: Library[] = this.cfg.libraries.map((l) => {
      const folders = l.folders.map((f) => this.folderRef({ ...f, libraryId: l.id, temp: false, expiresAt: null, sourceName: null }));
      return { ...l, folders, mediaCount: folders.reduce((a, f) => a + f.mediaCount, 0) };
    });
    const temp = this.cfg.temp.map((t) => this.folderRef({ ...t, libraryId: null, temp: true, expiresAt: t.expiresAt, sourceName: t.sourceName }));
    return { libraries, temp };
  }

  async createLibrary(input: { name: string; color?: string; icon?: string; folders?: string[] }) {
    const n = this.cfg.libraries.length;
    const lib = {
      id: `l${this.cfg.seq++}`,
      name: input.name || 'Library',
      color: input.color ?? COLORS[n % COLORS.length],
      icon: input.icon ?? ICONS[n % ICONS.length],
      sort: n,
      folders: (input.folders ?? []).map((p) => ({ id: `f${this.cfg.seq++}`, path: p.replace(/\/+$/, ''), name: basename(p) || p })),
    };
    this.cfg.libraries.push(lib);
    this.save();
    this.emit({ type: 'folders' });
    return (await this.libraries()).libraries.find((l) => l.id === lib.id)!;
  }

  async updateLibrary(id: string, patch: { name?: string; color?: string; icon?: string; sort?: number }) {
    const l = this.cfg.libraries.find((x) => x.id === id);
    if (!l) throw new ApiError(404, 'Library not found');
    Object.assign(l, Object.fromEntries(Object.entries(patch).filter(([, v]) => v !== undefined)));
    this.save();
    this.emit({ type: 'folders' });
    return (await this.libraries()).libraries.find((x) => x.id === id)!;
  }

  async deleteLibrary(id: string) {
    this.cfg.libraries = this.cfg.libraries.filter((l) => l.id !== id);
    this.save();
    this.emit({ type: 'folders' });
  }

  async addFolder(libraryId: string, path: string, name?: string) {
    const l = this.cfg.libraries.find((x) => x.id === libraryId);
    if (!l) throw new ApiError(404, 'Library not found');
    const f = { id: `f${this.cfg.seq++}`, path: path.replace(/\/+$/, ''), name: name || basename(path) };
    l.folders.push(f);
    this.save();
    this.emit({ type: 'folders' });
    return this.folderRef({ ...f, libraryId, temp: false, expiresAt: null, sourceName: null });
  }

  async renameFolder(folderId: string, name: string) {
    for (const l of this.cfg.libraries) for (const f of l.folders) if (f.id === folderId) f.name = name;
    this.save();
    this.emit({ type: 'folders' });
  }

  async removeFolder(folderId: string) {
    const t = this.cfg.temp.find((x) => x.id === folderId);
    if (t) {
      await PeekLocal.remove({ paths: [dirname(t.path)], permanent: true }).catch(() => {});
      this.cfg.temp = this.cfg.temp.filter((x) => x.id !== folderId);
    }
    for (const l of this.cfg.libraries) l.folders = l.folders.filter((f) => f.id !== folderId);
    this.save();
    this.emit({ type: 'folders' });
  }

  async rescanFolder() {
    this.cache.clear();
    this.emit({ type: 'media', folderId: '*', dirs: ['*'] });
  }

  async hostDirs(path?: string) {
    await this.ready;
    if (!path) {
      const { roots } = await PeekLocal.roots();
      return { path: null, parent: null, dirs: roots };
    }
    const { dirs } = await PeekLocal.listDirs({ path });
    const parent = path.split('/').length > 4 ? dirname(path) : null; // stop at /storage/emulated/0
    return { path, parent, dirs };
  }

  async hostMkdir(path: string, name: string) {
    return PeekLocal.mkdir({ path, name });
  }

  // ───────────────────────── browsing ─────────────────────────

  async browse(q: BrowseQuery): Promise<BrowseResult> {
    const all = await this.browseAll(q);
    const offset = q.offset ?? 0;
    const limit = q.limit ?? 600;
    return { ...all, folders: offset ? [] : all.folders, archives: offset ? [] : all.archives, items: all.items.slice(offset, offset + limit) };
  }

  async skeleton(q: BrowseQuery): Promise<BrowseSkeleton> {
    const all = await this.browseAll(q);
    return buildSkeleton(all.items, q);
  }

  async explore(): Promise<ExploreResult> {
    await this.ready;
    return buildExplore(await this.collect(this.allFolders().filter((f) => !f.temp)));
  }

  /** calendar heatmap: items per day of a year */
  async calendar(year: number): Promise<CalendarYear> {
    await this.ready;
    return calendarFor(await this.collect(this.allFolders().filter((f) => !f.temp)), year);
  }

  async geoPoints(q: BrowseQuery): Promise<GeoPoints> {
    const all = await this.browseAll(q);
    return geoPointsFor(all.items);
  }

  /** browse without paging: the full, filtered and sorted item list */
  private async browseAll(q: BrowseQuery): Promise<BrowseResult> {
    await this.ready;
    const libs = await this.libraries();
    let lib = q.libraryId ? libs.libraries.find((l) => l.id === q.libraryId) : undefined;
    let folderId = q.folderId;
    if (!folderId && lib?.folders.length === 1) folderId = lib.folders[0].id;

    let items: MediaItem[] = [];
    let folders: SubFolder[] = [];
    let archives: MediaItem[] = [];
    let crumbs: Crumb[] = [];
    let roots: FolderRef[] = [];
    let target: BrowseResult['target'] = null;
    const recursive = !!q.recursive || !!q.q;

    if (folderId) {
      const f = this.folder(folderId);
      if (!lib && f.libraryId) lib = libs.libraries.find((l) => l.id === f.libraryId);
      const dir = q.dir ?? '';
      const abs = join(f.path, dir);
      const data = await this.list(abs, recursive);
      const all = data.files.map((n) => this.toItem(n, f));
      items = all.filter((i) => i.kind !== 'archive');
      archives = recursive ? [] : all.filter((i) => i.kind === 'archive');
      folders = data.dirs.map((d) => ({ name: d.name, dir: join(dir, d.name), mediaCount: d.count, coverIds: d.covers.map(encId) }));
      if (!dir && !recursive) {
        this.cfg.counts[f.id] = items.filter((i) => i.kind !== 'audio').length + folders.reduce((a, s) => a + s.mediaCount, 0);
        this.save();
      }
      if (lib) crumbs.push({ label: lib.name, libraryId: lib.id });
      if (!lib || lib.folders.length > 1 || f.temp) crumbs.push({ label: f.name, libraryId: lib?.id, folderId: f.id, dir: '' });
      let acc = '';
      for (const seg of dir.split('/').filter(Boolean)) {
        acc = join(acc, seg);
        crumbs.push({ label: seg, libraryId: lib?.id, folderId: f.id, dir: acc });
      }
      target = { folderId: f.id, dir };
      if (!recursive) void PeekLocal.watch({ path: abs }).catch(() => {});
    } else if (lib) {
      crumbs = [{ label: lib.name, libraryId: lib.id }];
      roots = lib.folders;
      if (recursive) items = await this.collect(lib.folders.map((f) => this.folder(f.id)));
    } else {
      items = await this.collect(this.allFolders().filter((f) => !f.temp));
    }

    if (q.kind === 'image') items = items.filter((i) => i.kind === 'image');
    else if (q.kind === 'video') items = items.filter((i) => i.kind === 'video');
    else if (q.kind === 'favorites') items = items.filter((i) => i.favorite && i.kind !== 'audio');
    // free-text search (name, place, camera, type and date words) is handled by filterExplore below
    const dirMul = q.order === 'asc' ? 1 : -1;
    const key = q.sort ?? 'takenAt';
    items.sort((a, b) => {
      const va = key === 'name' ? a.name.toLocaleLowerCase() : key === 'kind' ? a.kind : a[key];
      const vb = key === 'name' ? b.name.toLocaleLowerCase() : key === 'kind' ? b.kind : b[key];
      return (va < vb ? -1 : va > vb ? 1 : 0) * dirMul;
    });
    if (q.from != null) items = items.filter((i) => i.localAt >= q.from!);
    if (q.to != null) items = items.filter((i) => i.localAt < q.to!);
    items = filterExplore(items, q);
    return { crumbs, folders, roots, items, total: items.length, archives, target };
  }

  private async collect(fs: ReturnType<AndroidLocalBackend['folder']>[]) {
    const out: MediaItem[] = [];
    for (const f of fs) {
      const data = await this.list(f.path, true).catch(() => null);
      if (data) out.push(...data.files.filter((n) => n.kind !== 'archive').map((n) => this.toItem(n, f)));
    }
    return out;
  }

  async timeline(): Promise<{ buckets: TimelineBucket[] }> {
    const items = await this.collect(this.allFolders().filter((f) => !f.temp));
    const m = new Map<string, number>();
    for (const i of items) {
      if (i.kind === 'audio') continue;
      const d = new Date(i.takenAt);
      const k = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
      m.set(k, (m.get(k) ?? 0) + 1);
    }
    return { buckets: [...m.entries()].sort((a, b) => (a[0] < b[0] ? 1 : -1)).map(([key, count]) => ({ key, count })) };
  }

  async media(id: string): Promise<MediaDetails> {
    await this.ready;
    const path = decId(id);
    const d = await PeekLocal.details({ path });
    const f = this.allFolders().find((x) => path.startsWith(x.path + '/')) ?? { id: '', path: dirname(path) };
    const item = this.toItem(d, f);
    return { ...item, path, exif: (d.exif as MediaDetails['exif']) ?? null, gps: null, place: null, camera: null, tzOffset: null };
  }

  async setFavorite(ids: string[], favorite: boolean) {
    const set = new Set(this.cfg.favorites);
    for (const id of ids) {
      const p = decId(id);
      if (favorite) set.add(p);
      else set.delete(p);
    }
    this.cfg.favorites = [...set];
    this.save();
    this.cache.clear();
    this.emit({ type: 'media', folderId: '*', dirs: ['*'] });
  }

  // ───────────────────────── URLs (served by the in-WebView media server) ─────────────────────────

  private peek(kind: string, path: string, extra = '') {
    return `/_peek/${kind}?p=${encodeURIComponent(path)}${extra}`;
  }
  thumbUrl(id: string, size: number) {
    const ms = this.msIds.get(id);
    return this.peek('thumb', decId(id), `&s=${size <= 320 ? 320 : size <= 640 ? 640 : 1600}${ms != null ? `&id=${ms}` : ''}`);
  }
  fileUrl(id: string) {
    return this.peek('file', decId(id));
  }
  displayUrl(item: Pick<MediaItem, 'id' | 'playback' | 'kind'>) {
    return item.playback === 'convert' ? this.peek('display', decId(item.id)) : this.fileUrl(item.id);
  }
  zoomUrl(item: Pick<MediaItem, 'id' | 'playback' | 'kind'>) {
    // /_peek/display renders a JPEG with the longest side ≤ 4096
    return this.peek('display', decId(item.id));
  }
  streamUrl(id: string) {
    return this.fileUrl(id);
  }
  previewUrl(id: string) {
    return this.fileUrl(id);
  }
  filmstripUrl(id: string, n: number, h: number) {
    return this.peek('strip', decId(id), `&n=${n}&h=${h}`);
  }
  downloadUrl() {
    return '';
  }
  folderDownloadUrl() {
    return '';
  }

  /** "Download" on the phone = system share sheet. */
  async share(ids: string[]) {
    await PeekLocal.share({ paths: ids.map(decId) });
  }

  // ───────────────────────── adding files ─────────────────────────

  private tmp(name: string) {
    return `${this.dirs.cacheDir}/uploads/${rid()}-${name}`;
  }

  async upload(file: Blob, opts: UploadOptions): Promise<UploadResult> {
    await this.ready;
    const f = this.folder(opts.folderId);
    const rel = opts.path.replace(/^\/+/, '');
    const name = basename(rel);
    const destDir = join(f.path, opts.dir, rel.includes('/') ? rel.slice(0, rel.lastIndexOf('/')) : '');
    const tmp = this.tmp(name);
    await writeBlob(tmp, file, opts.onProgress);
    this.cache.clear();
    if (extOf(name) === 'zip' && opts.zip && opts.zip !== 'keep') return this.unzip(tmp, name, opts.zip, destDir, true);
    const r = await PeekLocal.finishFile({ tmp, dir: destDir, name });
    const ref = this.refFor(r.path);
    return { created: ref ? [ref] : [] };
  }

  private async unzip(zipPath: string, name: string, mode: UploadZipMode, destDir: string, deleteZip: boolean): Promise<UploadResult> {
    if (mode === 'temp') {
      const session = `${this.dirs.cacheDir}/temp/${rid()}`;
      const r = await PeekLocal.extractZip({ zip: zipPath, dest: `${session}/content`, unique: false });
      if (deleteZip) await PeekLocal.remove({ paths: [zipPath], permanent: true });
      const t = { id: `t${this.cfg.seq++}`, path: r.path, name: name.replace(/\.zip$/i, ''), sourceName: name, expiresAt: Date.now() + this.cfg.settings.tempSessionHours * 3600_000 };
      this.cfg.temp.unshift(t);
      this.save();
      this.emit({ type: 'folders' });
      return { created: [], tempFolder: this.folderRef({ ...t, libraryId: null, temp: true, expiresAt: t.expiresAt, sourceName: t.sourceName }) };
    }
    const r = await PeekLocal.extractZip({ zip: zipPath, dest: destDir, unique: true });
    if (deleteZip) await PeekLocal.remove({ paths: [zipPath], permanent: true });
    this.cache.clear();
    return { created: [], extractedDir: this.refFor(r.path + '/x')?.dir };
  }

  async importLocal(): Promise<UploadResult> {
    throw new ApiError(400, 'Not available on the phone');
  }

  async peekZip(id: string) {
    await this.ready;
    const p = decId(id);
    const r = await this.unzip(p, basename(p), 'temp', '', false);
    return { tempFolder: r.tempFolder! };
  }

  async extractZip(id: string) {
    const p = decId(id);
    const r = await PeekLocal.extractZip({ zip: p, dest: dirname(p), unique: true });
    this.cache.clear();
    const ref = this.refFor(r.path + '/x');
    return { folderId: ref?.folderId ?? '', dir: ref?.dir ?? '' };
  }

  async keepTemp(tempId: string, target: { folderId: string; dir: string }) {
    const t = this.cfg.temp.find((x) => x.id === tempId);
    if (!t) throw new ApiError(404, 'Not a temporary session');
    const dest = join(this.folder(target.folderId).path, target.dir);
    // move the whole extracted folder (incl. non-media files), then give it the archive's name
    const moved = (await PeekLocal.move({ paths: [t.path], dest })).paths[0];
    let finalPath = moved;
    for (let i = 0; i < 50; i++) {
      try {
        finalPath = (await PeekLocal.rename({ path: moved, name: i ? `${t.name} (${i})` : t.name })).path;
        break;
      } catch {
        /* name taken → try the next one */
      }
    }
    const dir = { path: finalPath };
    await this.removeFolder(tempId);
    this.cache.clear();
    const ref = this.refFor(dir.path + '/x')!;
    return { folderId: ref.folderId, dir: ref.dir };
  }

  async extendTemp(tempId: string, hours: number) {
    const t = this.cfg.temp.find((x) => x.id === tempId);
    if (t) t.expiresAt = Date.now() + hours * 3600_000;
    this.save();
    this.emit({ type: 'folders' });
  }

  // ───────────────────────── file ops ─────────────────────────

  private abs(ref: FileRef) {
    return join(this.folder(ref.folderId).path, ref.rel);
  }

  async mkdir(folderId: string, dir: string, name: string) {
    const r = await PeekLocal.mkdir({ path: join(this.folder(folderId).path, dir), name });
    this.cache.clear();
    const ref = this.refFor(r.path + '/x')!;
    return { folderId: ref.folderId, dir: ref.dir };
  }

  async rename(ref: FileRef, name: string) {
    const r = await PeekLocal.rename({ path: this.abs(ref), name });
    this.cache.clear();
    this.emit({ type: 'media', folderId: ref.folderId, dirs: ['*'] });
    return { rel: r.path.slice(this.folder(ref.folderId).path.length + 1) };
  }

  async move(refs: FileRef[], to: { folderId: string; dir: string }, copy = false) {
    await PeekLocal.move({ paths: refs.map((r) => this.abs(r)), dest: join(this.folder(to.folderId).path, to.dir), copy });
    this.cache.clear();
    this.emit({ type: 'media', folderId: '*', dirs: ['*'] });
  }

  async remove(refs: FileRef[]) {
    const temp = refs.some((r) => this.cfg.temp.some((t) => t.id === r.folderId));
    await PeekLocal.remove({ paths: refs.map((r) => this.abs(r)), permanent: temp });
    this.cache.clear();
    this.emit({ type: 'media', folderId: '*', dirs: ['*'] });
  }

  // ───────────────────────── fonts ─────────────────────────

  private fontInfo(): FontInfo[] {
    return this.cfg.fonts.map((f) => ({ id: f.id, family: f.family, subfamily: f.subfamily, weight: f.weight, style: f.style, format: f.format, url: `/_peek/file?p=${encodeURIComponent(f.path)}`, bundled: false }));
  }

  async fonts() {
    return this.fontInfo();
  }

  async uploadFont(file: File) {
    await this.ready;
    const ext = extOf(file.name);
    if (!['ttf', 'otf', 'woff', 'woff2'].includes(ext)) throw new ApiError(400, 'Unsupported font format');
    const info = readFontInfo(await file.arrayBuffer(), file.name);
    const path = `${this.dirs.filesDir}/fonts/${rid()}.${ext}`;
    await writeBlob(path, file);
    this.cfg.fonts.push({
      id: `font${this.cfg.seq++}`,
      family: info.family,
      subfamily: info.subfamily,
      weight: info.weight,
      style: info.italic ? 'italic' : 'normal',
      format: ext === 'ttf' ? 'truetype' : ext === 'otf' ? 'opentype' : (ext as 'woff' | 'woff2'),
      path,
    });
    this.save();
    this.emit({ type: 'fonts' });
    return this.fontInfo();
  }

  async deleteFont(id: string) {
    const f = this.cfg.fonts.find((x) => x.id === id);
    if (f) await PeekLocal.remove({ paths: [f.path], permanent: true }).catch(() => {});
    this.cfg.fonts = this.cfg.fonts.filter((x) => x.id !== id);
    this.save();
    this.emit({ type: 'fonts' });
    return this.fontInfo();
  }

  fontUrl(font: FontInfo) {
    return font.url;
  }

  // ───────────────────────── edits & jobs ─────────────────────────

  async saveEdit(image: Blob, opts: SaveEditOptions) {
    await this.ready;
    const tmp = this.tmp('edit.png');
    await writeBlob(tmp, image);
    const r = await PeekLocal.saveImage({
      tmp,
      format: opts.format,
      quality: opts.quality ?? 92,
      source: opts.sourceId ? decId(opts.sourceId) : undefined,
      suffix: this.cfg.settings.editSuffix,
      dir: opts.folderId ? join(this.folder(opts.folderId).path, opts.dir ?? '') : undefined,
      name: opts.name,
    });
    this.cache.clear();
    this.emit({ type: 'media', folderId: '*', dirs: ['*'] });
    const ref = this.refFor(r.path);
    return { item: ref ?? { folderId: '', dir: '', name: basename(r.path) }, id: encId(r.path) };
  }

  async jobs() {
    return [...this.jobsById.values()].sort((a, b) => b.createdAt - a.createdAt);
  }

  async cancelJob(id: string) {
    await PeekLocal.cancelJob({ id });
  }

  async clearJobs() {
    for (const [id, j] of this.jobsById) if (j.status !== 'running' && j.status !== 'queued') this.jobsById.delete(id);
  }

  async uploadJobAsset(blob: Blob, ext = 'png') {
    await this.ready;
    const path = `${this.dirs.cacheDir}/job-assets/${rid()}.${ext}`;
    await writeBlob(path, blob);
    return encId(path);
  }

  private track(id: string, type: Job['type'], title: string): Job {
    const j: Job = { id, type, title, status: 'running', progress: 0, error: null, result: null, createdAt: Date.now() };
    this.jobsById.set(id, j);
    this.emit({ type: 'job', job: j });
    return j;
  }

  async startVideoEdit(spec: VideoEditSpec) {
    await this.ready;
    if (spec.output.format !== 'mp4') throw new ApiError(400, 'Telefonda yalnızca MP4 dışa aktarılabilir / Only MP4 export is available on the phone');
    const src = await this.media(spec.sourceId);
    const r = await PeekLocal.videoEdit({
      input: src.path,
      srcWidth: src.width ?? 0,
      srcHeight: src.height ?? 0,
      trim: spec.trim,
      crop: spec.crop,
      rotate: spec.rotate ?? 0,
      flipH: !!spec.flipH,
      flipV: !!spec.flipV,
      speed: spec.speed ?? 1,
      volume: spec.volume ?? 1,
      mute: !!spec.mute,
      adjust: spec.adjust ?? {},
      overlays: (spec.overlays ?? []).map((o) => ({ path: decId(o.assetId), start: o.start, end: o.end })),
      height: spec.output.height ?? 0,
      suffix: this.cfg.settings.editSuffix,
    });
    return this.track(r.id, 'video-edit', src.name);
  }

  async startSlideshow(spec: SlideshowExportSpec) {
    await this.ready;
    const items = spec.items.map((it) => {
      const path = decId(it.id);
      const ext = extOf(path);
      return { path, kind: ['mp4', 'm4v', 'mov', 'webm', 'mkv', '3gp'].includes(ext) ? 'video' : 'image', duration: it.duration ?? spec.duration, trim: it.trim };
    });
    const dir = spec.target ? join(this.folder(spec.target.folderId).path, spec.target.dir) : dirname(items[0].path);
    const r = await PeekLocal.slideshow({
      items,
      duration: spec.duration,
      transition: spec.transition,
      transitionDuration: spec.transitionDuration,
      kenBurns: spec.kenBurns,
      audio: spec.audioId ? decId(spec.audioId) : '',
      width: spec.width,
      height: spec.height,
      fps: spec.fps,
      dir,
      name: spec.name ?? 'Slayt',
    });
    return this.track(r.id, 'slideshow', spec.name ?? 'Slayt');
  }

  async cacheSize() {
    return (await PeekLocal.cacheSize()).bytes;
  }

  async clearCache() {
    await PeekLocal.clearCache();
  }

  subscribe(fn: (e: ServerEvent) => void) {
    this.listeners.add(fn);
    return () => {
      this.listeners.delete(fn);
    };
  }

  onConnection(fn: (s: ConnectionState) => void) {
    fn('online');
    return () => {};
  }

  dispose() {
    this.listeners.clear();
    for (const h of this.handles) void h.remove();
  }
}
