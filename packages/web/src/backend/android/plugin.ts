import { registerPlugin, type PluginListenerHandle } from '@capacitor/core';

export interface NativeFile {
  path: string;
  name: string;
  kind: 'image' | 'video' | 'audio' | 'archive';
  size: number;
  mtime: number;
  width: number | null;
  height: number | null;
  duration: number | null;
  takenAt: number;
  /** MediaStore _ID when the file is indexed by the system (fast thumbnails), else null */
  id?: number | null;
}

export interface NativeDir {
  name: string;
  path: string;
  count: number;
  covers: string[];
}

export interface SharedFile {
  uri: string;
  name: string;
  size: number;
  mime: string | null;
}

/** 'granted' | 'denied' | 'prompt'; permissions that do not exist on this Android version report 'granted' */
export type NativePermState = 'granted' | 'denied' | 'prompt';

export interface MediaPermissions {
  /** READ_MEDIA_IMAGES + READ_MEDIA_VIDEO (Android 13+) */
  media: NativePermState;
  /** ACCESS_MEDIA_LOCATION (Android 10+): un-redacted EXIF GPS */
  location: NativePermState;
  /** POST_NOTIFICATIONS (Android 13+): link service notification, transfer alerts */
  notifications: NativePermState;
}

/**
 * Progress of a native transfer (downloadTo / uploadPaths). Throttled to ~4/s per file.
 * The last event of a transfer has `done: true` (+ `error` on failure / 'canceled'), and
 * `paths` (downloads: absolute paths written) or `results` (uploads: the server's UploadResult per file).
 */
export interface NativeTransferEvent {
  id: string;
  kind: 'download' | 'upload';
  /** current file name (uploads: relative path sent as `path`); '' on the final event */
  file: string;
  /** 0-based index of the current file (final event: number of files done) */
  index: number;
  count: number;
  /** bytes of the current file so far / its size (-1 = unknown) */
  bytes: number;
  total: number;
  done: boolean;
  error?: string;
  paths?: string[];
  results?: Record<string, unknown>[];
}

/** PeekIndex progress (also returned by indexStatus / indexSync) */
export interface NativeIndexStatus {
  done: number;
  total: number;
  running: boolean;
}

/**
 * indexQuery result: parallel arrays, one entry per indexed file.
 * flags bits: 1 = XMP GPano panorama, 2 = animated (GIF/WebP with frames) — map to MEDIA_FLAG with shared `classify`.
 */
export interface NativeIndexColumns {
  path: string[];
  lat: (number | null)[];
  lon: (number | null)[];
  make: (string | null)[];
  model: (string | null)[];
  lens: (string | null)[];
  /** capture-time UTC offset in minutes (EXIF OffsetTimeOriginal) */
  offset: (number | null)[];
  flags: number[];
}

export interface NativeJobEvent {
  id: string;
  title: string;
  type: 'video-edit' | 'slideshow';
  status: 'running' | 'done' | 'error' | 'canceled';
  progress: number;
  error: string | null;
  resultPath?: string;
}

/** Kotlin plugin: packages/android/android/app/src/main/java/app/peekaround/PeekLocalPlugin.kt */
export interface PeekLocalPlugin {
  /** granted = all-files access (Android 11+) / legacy storage permission; media = runtime permission states */
  checkAccess(): Promise<{ granted: boolean; sdk: number; media: MediaPermissions }>;
  /** Ask for READ_MEDIA_IMAGES/VIDEO (13+), ACCESS_MEDIA_LOCATION (10+) and POST_NOTIFICATIONS (13+) — call from a user gesture. */
  requestMediaPermissions(): Promise<MediaPermissions>;
  /**
   * Restrict http://localhost/_peek/* to these folders (library folders + temp sessions; canonicalised,
   * symlinks resolved). The app cache/files dirs and folders shared with a PC are always allowed.
   * Before the first call the storage roots of roots() are allowed. Pass `paths: null` to reset. Others → 403.
   */
  setAllowedRoots(o: { paths: string[] | null }): Promise<void>;
  /** Start / restart the background metadata indexer (GPS, camera, offset, panorama) over these folder roots. Incremental. */
  indexSync(o: { paths: string[] }): Promise<NativeIndexStatus>;
  indexStatus(): Promise<NativeIndexStatus>;
  /** Indexed rows below these folder roots (or exact file paths), columnar. */
  indexQuery(o: { paths: string[] }): Promise<NativeIndexColumns>;
  /**
   * Native streaming download of server files into a phone folder (unique names, `.part` → rename,
   * mtime = item.mtime ?? X-Peek-Mtime ?? Last-Modified, MediaScanner). Resolves at once with the
   * transfer id; follow 'transfer' events. item.dir = sub folder (relative, created) under dest.
   * headers: sent with every GET (e.g. Authorization / X-Peek-Device from PeekLink.status()).
   */
  downloadTo(o: { id?: string; items: { url: string; name: string; mtime?: number | null; dir?: string }[]; dest: string; headers?: Record<string, string> }): Promise<{ id: string }>;
  /**
   * Native streaming upload: PUT `${url}/api/upload?folderId&dir&path&mtime[&zip]` per file (X-Peek: 1, Content-Length
   * from disk). paths: absolute files, folders (recursive; path = "<folder>/<rel>") or content:// URIs.
   * Resolves at once with the transfer id; follow 'transfer' events (final event carries `results`).
   */
  uploadPaths(o: { id?: string; url: string; folderId: string; dir?: string; paths: string[]; zip?: string; headers?: Record<string, string> }): Promise<{ id: string }>;
  cancelTransfer(o: { id: string }): Promise<void>;
  requestAccess(): Promise<{ granted: boolean; pending?: boolean }>;
  roots(): Promise<{ roots: { name: string; path: string }[]; cacheDir: string; filesDir: string }>;
  list(o: { path: string; recursive?: boolean; showHidden?: boolean; limit?: number }): Promise<{ path: string; files: NativeFile[]; dirs: NativeDir[]; truncated: boolean }>;
  listDirs(o: { path: string }): Promise<{ dirs: { name: string; path: string }[] }>;
  details(o: { path: string }): Promise<NativeFile & { exif?: Record<string, string | number> }>;
  /** creates path/name (unique "name (1)" when taken); rejects for a bad name or a missing parent */
  mkdir(o: { path: string; name: string }): Promise<{ path: string; name: string }>;
  rename(o: { path: string; name: string }): Promise<{ path: string }>;
  move(o: { paths: string[]; dest: string; copy?: boolean }): Promise<{ paths: string[] }>;
  remove(o: { paths: string[]; permanent?: boolean }): Promise<{ deleted: number }>;
  writeChunk(o: { path: string; data: string; append: boolean }): Promise<void>;
  finishFile(o: { tmp: string; dir: string; name: string }): Promise<{ path: string }>;
  saveImage(o: { tmp: string; format: string; quality: number; source?: string; suffix?: string; dir?: string; name?: string }): Promise<{ path: string }>;
  extractZip(o: { zip: string; dest: string; unique?: boolean }): Promise<{ path: string; files: number }>;
  cacheSize(): Promise<{ bytes: number }>;
  clearCache(): Promise<void>;
  share(o: { paths: string[]; mime?: string }): Promise<void>;
  watch(o: { path?: string }): Promise<void>;
  videoEdit(o: Record<string, unknown>): Promise<{ id: string; path: string }>;
  slideshow(o: Record<string, unknown>): Promise<{ id: string; path: string }>;
  cancelJob(o: { id: string }): Promise<void>;
  discover(o?: { timeout?: number }): Promise<{ servers: { name: string; host: string; port: number; url: string }[] }>;
  /** files shared to the app, and/or `text` (ACTION_SEND text/plain, e.g. a mega.nz link) */
  getShared(): Promise<{ files: SharedFile[]; text: string | null }>;
  clearShared(): Promise<void>;
  uploadShared(o: { url: string; folderId: string; dir: string; zip?: string; files: SharedFile[]; headers?: Record<string, string> }): Promise<{ uploaded: number }>;
  importShared(o: { dir: string; files: SharedFile[] }): Promise<{ paths: string[] }>;
  addListener(event: 'job', fn: (e: NativeJobEvent) => void): Promise<PluginListenerHandle>;
  addListener(event: 'changed', fn: (e: { path: string }) => void): Promise<PluginListenerHandle>;
  addListener(event: 'shared', fn: (e: { count: number; text: string | null }) => void): Promise<PluginListenerHandle>;
  addListener(event: 'transfer', fn: (e: NativeTransferEvent) => void): Promise<PluginListenerHandle>;
  /** throttled to ~1/s while running, plus a final event with running: false */
  addListener(event: 'index', fn: (e: NativeIndexStatus) => void): Promise<PluginListenerHandle>;
  addListener(event: 'uploadProgress', fn: (e: { index: number; name: string; sent: number; size: number; total: number }) => void): Promise<PluginListenerHandle>;
  addListener(event: 'download', fn: (e: { name: string }) => void): Promise<PluginListenerHandle>;
}

export const PeekLocal = registerPlugin<PeekLocalPlugin>('PeekLocal');

export function isNative(): boolean {
  const w = window as Window & { Capacitor?: { isNativePlatform?: () => boolean } };
  return !!w.Capacitor?.isNativePlatform?.();
}

/** Blob → native file, in base64 chunks (the bridge only carries strings). */
export async function writeBlob(path: string, blob: Blob, onProgress?: (done: number, total: number) => void) {
  const CHUNK = 1024 * 1024 * 1.5;
  if (blob.size === 0) {
    await PeekLocal.writeChunk({ path, data: '', append: false });
    return;
  }
  for (let off = 0; off < blob.size; off += CHUNK) {
    const buf = new Uint8Array(await blob.slice(off, off + CHUNK).arrayBuffer());
    let bin = '';
    for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
    await PeekLocal.writeChunk({ path, data: btoa(bin), append: off > 0 });
    onProgress?.(Math.min(blob.size, off + CHUNK), blob.size);
  }
}
