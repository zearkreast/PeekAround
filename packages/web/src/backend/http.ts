import type {
  BrowseQuery,
  BrowseResult,
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
  TimelineBucket,
  UploadResult,
  UploadZipMode,
  VideoEditSpec,
} from '@peekaround/shared';
import { ApiError, type Backend, type ConnectionState, type SaveEditOptions, type UploadOptions } from './types';

function qs(params: Record<string, string | number | boolean | undefined | null>): string {
  const u = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === '' || v === false) continue;
    u.set(k, v === true ? '1' : String(v));
  }
  const s = u.toString();
  return s ? `?${s}` : '';
}

/** Talks to a PeekAround server over HTTP + a websocket for live events. */
export class HttpBackend implements Backend {
  readonly kind = 'http' as const;
  readonly id: string;
  private ws: WebSocket | null = null;
  private listeners = new Set<(e: ServerEvent) => void>();
  private connListeners = new Set<(s: ConnectionState) => void>();
  private conn: ConnectionState = 'connecting';
  private retry = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;

  /** @param base '' for same origin, or e.g. "http://192.168.1.20:8080" */
  constructor(
    readonly base: string,
    readonly label = 'PeekAround',
  ) {
    this.base = base.replace(/\/+$/, '');
    this.id = `http:${this.base || location.origin}`;
  }

  private url(p: string) {
    return `${this.base}${p}`;
  }

  private async req<T>(method: string, p: string, body?: unknown, init: RequestInit = {}): Promise<T> {
    let res: Response;
    try {
      res = await fetch(this.url(p), {
        method,
        headers: body !== undefined && !(body instanceof Blob) ? { 'Content-Type': 'application/json' } : undefined,
        body: body === undefined ? undefined : body instanceof Blob ? body : JSON.stringify(body),
        ...init,
      });
    } catch (e) {
      throw new ApiError(0, (e as Error).message || 'Network error');
    }
    if (!res.ok) {
      let msg = res.statusText;
      try {
        const j = await res.json();
        msg = j.error ?? msg;
      } catch {
        /* not json */
      }
      throw new ApiError(res.status, msg);
    }
    const ct = res.headers.get('content-type') ?? '';
    return (ct.includes('application/json') ? res.json() : (undefined as T)) as Promise<T>;
  }

  info() {
    return this.req<ServerInfo>('GET', '/api/info');
  }
  settings() {
    return this.req<Settings>('GET', '/api/settings');
  }
  updateSettings(patch: Partial<Settings>) {
    return this.req<Settings>('PATCH', '/api/settings', patch);
  }

  libraries() {
    return this.req<{ libraries: Library[]; temp: FolderRef[] }>('GET', '/api/libraries');
  }
  createLibrary(input: { name: string; color?: string; icon?: string; folders?: string[] }) {
    return this.req<Library>('POST', '/api/libraries', input);
  }
  updateLibrary(id: string, patch: { name?: string; color?: string; icon?: string; sort?: number }) {
    return this.req<Library>('PATCH', `/api/libraries/${id}`, patch);
  }
  async deleteLibrary(id: string) {
    await this.req('DELETE', `/api/libraries/${id}`);
  }
  addFolder(libraryId: string, path: string, name?: string) {
    return this.req<FolderRef>('POST', `/api/libraries/${libraryId}/folders`, { path, name });
  }
  async renameFolder(folderId: string, name: string) {
    await this.req('PATCH', `/api/folders/${folderId}`, { name });
  }
  async removeFolder(folderId: string) {
    await this.req('DELETE', `/api/folders/${folderId}`);
  }
  async rescanFolder(folderId: string) {
    await this.req('POST', `/api/folders/${folderId}/rescan`, {});
  }
  hostDirs(path?: string) {
    return this.req<{ path: string | null; parent: string | null; dirs: { name: string; path: string }[] }>('GET', `/api/host/dirs${qs({ path })}`);
  }
  hostMkdir(path: string, name: string) {
    return this.req<{ path: string }>('POST', '/api/host/mkdir', { path, name });
  }

  browse(q: BrowseQuery) {
    return this.req<BrowseResult>(
      'GET',
      `/api/browse${qs({
        libraryId: q.libraryId,
        folderId: q.folderId,
        dir: q.dir,
        recursive: q.recursive,
        sort: q.sort,
        order: q.order,
        kind: q.kind,
        q: q.q,
        offset: q.offset,
        limit: q.limit,
      })}`,
    );
  }
  timeline(libraryId?: string) {
    return this.req<{ buckets: TimelineBucket[] }>('GET', `/api/timeline${qs({ libraryId })}`);
  }
  media(id: string) {
    return this.req<MediaDetails>('GET', `/api/media/${id}`);
  }
  async setFavorite(ids: string[], favorite: boolean) {
    await this.req('POST', '/api/media/favorite', { ids, favorite });
  }

  thumbUrl(id: string, size: number) {
    return this.url(`/api/media/${id}/thumb?s=${size <= 320 ? 320 : size <= 640 ? 640 : 1600}`);
  }
  fileUrl(id: string, download = false) {
    return this.url(`/api/media/${id}/file${download ? '?download=1' : ''}`);
  }
  displayUrl(item: Pick<MediaItem, 'id' | 'playback' | 'kind'>) {
    return item.kind === 'image' && item.playback === 'convert' ? this.url(`/api/media/${item.id}/display`) : this.fileUrl(item.id);
  }
  streamUrl(id: string, startAt = 0) {
    return this.url(`/api/media/${id}/stream${startAt ? `?t=${startAt.toFixed(2)}` : ''}`);
  }
  previewUrl(id: string) {
    return this.url(`/api/media/${id}/preview`);
  }
  filmstripUrl(id: string, n: number, h: number) {
    return this.url(`/api/media/${id}/filmstrip?n=${n}&h=${h}`);
  }
  downloadUrl(ids: string[], name?: string) {
    return this.url(`/api/download${qs({ ids: ids.join(','), name })}`);
  }
  folderDownloadUrl(folderId: string, dir: string, name?: string) {
    return this.url(`/api/download${qs({ folderId, dir, name })}`);
  }

  upload(file: Blob, opts: UploadOptions): Promise<UploadResult> {
    return new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open('PUT', this.url(`/api/upload${qs({ folderId: opts.folderId, dir: opts.dir, path: opts.path, zip: opts.zip })}`));
      xhr.setRequestHeader('Content-Type', 'application/octet-stream');
      xhr.upload.onprogress = (e) => opts.onProgress?.(e.loaded, e.total || file.size);
      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          try {
            resolve(JSON.parse(xhr.responseText));
          } catch {
            resolve({ created: [] });
          }
        } else {
          let msg = xhr.statusText;
          try {
            msg = JSON.parse(xhr.responseText).error ?? msg;
          } catch {
            /* ignore */
          }
          reject(new ApiError(xhr.status, msg));
        }
      };
      xhr.onerror = () => reject(new ApiError(0, 'Network error'));
      xhr.onabort = () => reject(new ApiError(-1, 'Canceled'));
      opts.signal?.addEventListener('abort', () => xhr.abort(), { once: true });
      xhr.send(file);
    });
  }

  importLocal(paths: string[], folderId: string, dir: string, zip?: UploadZipMode) {
    return this.req<UploadResult>('POST', '/api/import-local', { paths, folderId, dir, zip });
  }
  peekZip(id: string) {
    return this.req<{ tempFolder: FolderRef }>('POST', '/api/zip/peek', { id });
  }
  extractZip(id: string) {
    return this.req<{ folderId: string; dir: string }>('POST', '/api/zip/extract', { id });
  }
  keepTemp(tempId: string, target: { folderId: string; dir: string }) {
    return this.req<{ folderId: string; dir: string }>('POST', `/api/temp/${tempId}/keep`, target);
  }
  async extendTemp(tempId: string, hours: number) {
    await this.req('POST', `/api/temp/${tempId}/extend`, { hours });
  }

  mkdir(folderId: string, dir: string, name: string) {
    return this.req<{ folderId: string; dir: string }>('POST', '/api/fs/mkdir', { folderId, dir, name });
  }
  rename(ref: FileRef, name: string) {
    return this.req<{ rel: string }>('POST', '/api/fs/rename', { ...ref, name });
  }
  async move(refs: FileRef[], to: { folderId: string; dir: string }, copy = false) {
    await this.req('POST', '/api/fs/move', { refs, to, copy });
  }
  async remove(refs: FileRef[]) {
    await this.req('POST', '/api/fs/delete', { refs });
  }

  async fonts() {
    return (await this.req<{ fonts: FontInfo[] }>('GET', '/api/fonts')).fonts;
  }
  async uploadFont(file: File) {
    return (await this.req<{ fonts: FontInfo[] }>('PUT', `/api/fonts${qs({ name: file.name })}`, file, { headers: { 'Content-Type': 'application/octet-stream' } })).fonts;
  }
  async deleteFont(id: string) {
    return (await this.req<{ fonts: FontInfo[] }>('DELETE', `/api/fonts/${id}`)).fonts;
  }
  fontUrl(font: FontInfo) {
    return font.bundled ? font.url : this.url(font.url);
  }

  saveEdit(image: Blob, opts: SaveEditOptions) {
    return this.req<{ item: { folderId: string; dir: string; name: string }; id: string | null }>(
      'POST',
      `/api/edits/save${qs({ sourceId: opts.sourceId, folderId: opts.folderId, dir: opts.dir, name: opts.name, format: opts.format, quality: opts.quality })}`,
      image,
      { headers: { 'Content-Type': 'application/octet-stream' } },
    );
  }

  async jobs() {
    return (await this.req<{ jobs: Job[] }>('GET', '/api/jobs')).jobs;
  }
  async cancelJob(id: string) {
    await this.req('DELETE', `/api/jobs/${id}`);
  }
  async clearJobs() {
    await this.req('DELETE', '/api/jobs');
  }
  async uploadJobAsset(blob: Blob, ext = 'png') {
    return (await this.req<{ assetId: string }>('PUT', `/api/jobs/assets?ext=${ext}`, blob, { headers: { 'Content-Type': 'application/octet-stream' } })).assetId;
  }
  startVideoEdit(spec: VideoEditSpec) {
    return this.req<Job>('POST', '/api/jobs/video-edit', spec);
  }
  startSlideshow(spec: SlideshowExportSpec) {
    return this.req<Job>('POST', '/api/jobs/slideshow', spec);
  }

  async cacheSize() {
    return (await this.req<{ bytes: number }>('GET', '/api/cache')).bytes;
  }
  async clearCache() {
    await this.req('DELETE', '/api/cache');
  }

  // ── live events ──

  subscribe(fn: (e: ServerEvent) => void) {
    this.listeners.add(fn);
    this.ensureSocket();
    return () => {
      this.listeners.delete(fn);
    };
  }

  onConnection(fn: (s: ConnectionState) => void) {
    this.connListeners.add(fn);
    fn(this.conn);
    return () => {
      this.connListeners.delete(fn);
    };
  }

  private setConn(s: ConnectionState) {
    if (this.conn === s) return;
    this.conn = s;
    for (const l of this.connListeners) l(s);
  }

  private ensureSocket() {
    if (this.ws || this.disposed) return;
    const httpBase = this.base || location.origin;
    const wsUrl = `${httpBase.replace(/^http/, 'ws')}/api/events`;
    const ws = new WebSocket(wsUrl);
    this.ws = ws;
    ws.onopen = () => {
      this.retry = 0;
      this.setConn('online');
    };
    ws.onmessage = (m) => {
      try {
        const e = JSON.parse(m.data) as ServerEvent;
        for (const l of this.listeners) l(e);
      } catch {
        /* ignore */
      }
    };
    ws.onclose = () => {
      this.ws = null;
      if (this.disposed) return;
      this.setConn(this.retry > 1 ? 'offline' : 'connecting');
      const delay = Math.min(15000, 500 * 2 ** this.retry++);
      this.retryTimer = setTimeout(() => this.ensureSocket(), delay);
    };
    ws.onerror = () => ws.close();
  }

  dispose() {
    this.disposed = true;
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.ws?.close();
    this.listeners.clear();
    this.connListeners.clear();
  }
}
