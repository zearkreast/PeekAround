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
  checkAccess(): Promise<{ granted: boolean; sdk: number }>;
  requestAccess(): Promise<{ granted: boolean; pending?: boolean }>;
  roots(): Promise<{ roots: { name: string; path: string }[]; cacheDir: string; filesDir: string }>;
  list(o: { path: string; recursive?: boolean; showHidden?: boolean; limit?: number }): Promise<{ path: string; files: NativeFile[]; dirs: NativeDir[]; truncated: boolean }>;
  listDirs(o: { path: string }): Promise<{ dirs: { name: string; path: string }[] }>;
  details(o: { path: string }): Promise<NativeFile & { exif?: Record<string, string | number> }>;
  mkdir(o: { path: string; name: string }): Promise<{ path: string }>;
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
  getShared(): Promise<{ files: SharedFile[] }>;
  clearShared(): Promise<void>;
  uploadShared(o: { url: string; folderId: string; dir: string; zip?: string; files: SharedFile[] }): Promise<{ uploaded: number }>;
  importShared(o: { dir: string; files: SharedFile[] }): Promise<{ paths: string[] }>;
  addListener(event: 'job', fn: (e: NativeJobEvent) => void): Promise<PluginListenerHandle>;
  addListener(event: 'changed', fn: (e: { path: string }) => void): Promise<PluginListenerHandle>;
  addListener(event: 'shared', fn: (e: { count: number }) => void): Promise<PluginListenerHandle>;
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
