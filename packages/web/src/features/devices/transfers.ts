/**
 * Native phone transfers (PeekLocal.downloadTo / uploadPaths) shown as 'transfer' jobs in the
 * jobs panel, next to the server's transfer jobs (copy to/from a linked phone, MEGA imports).
 */
import type { Job, MediaItem } from '@peekaround/shared';
import { joinRel } from '@peekaround/shared';
import type { Backend } from '@/backend';
import { HttpBackend } from '@/backend/http';
import { PeekLocal, type NativeTransferEvent } from '@/backend/android/plugin';
import { NATIVE_JOB, useJobs } from '@/store/jobs';
import { translate } from '@/i18n';
import { usePrefs } from '@/store/prefs';
import { go } from '@/lib/nav';
import { folderHref } from '@/lib/routes';
import { toast } from '@/store/toasts';
import { queryClient } from '@/lib/queryClient';

const t = (k: Parameters<typeof translate>[1], v?: Record<string, string | number>) => translate(usePrefs.getState().lang, k, v);

interface Meta {
  kind: 'download' | 'upload';
  title: string;
  /** byte size per file when known (downloads) */
  sizes: number[] | null;
  createdAt: number;
  /** upload destination on the server */
  target?: { folderId: string; dir: string };
  /** download destination label (phone path) */
  dest?: string;
}

const metas = new Map<string, Meta>();

/** Apply a native 'transfer' event to the jobs store. Returns the job. */
export function applyTransferEvent(e: NativeTransferEvent): Job {
  const meta = metas.get(e.id) ?? { kind: e.kind, title: e.file || 'Transfer', sizes: null, createdAt: Date.now() };
  const id = NATIVE_JOB + e.id;
  const sizes = meta.sizes;
  const count = Math.max(1, e.count);
  let bytes: Job['bytes'];
  if (sizes && sizes.length === e.count) {
    const total = sizes.reduce((a, b) => a + b, 0);
    const before = sizes.slice(0, Math.min(e.index, sizes.length)).reduce((a, b) => a + b, 0);
    bytes = { done: e.done && !e.error ? total : Math.min(total, before + Math.max(0, e.bytes)), total };
  }
  const fileFrac = e.total > 0 ? Math.min(1, Math.max(0, e.bytes) / e.total) : 0;
  const progress = e.done && !e.error ? 1 : bytes && bytes.total > 0 ? bytes.done / bytes.total : Math.min(1, (e.index + fileFrac) / count);
  const status: Job['status'] = !e.done ? 'running' : e.error === 'canceled' ? 'canceled' : e.error ? 'error' : 'done';
  const job: Job = {
    id,
    type: 'transfer',
    title: meta.title,
    status,
    progress,
    bytes,
    files: { done: e.index, total: e.count },
    error: status === 'error' ? (e.error ?? null) : null,
    result: meta.kind === 'upload' && meta.target ? { folderId: meta.target.folderId, dir: meta.target.dir, name: '' } : null,
    createdAt: meta.createdAt,
  };
  useJobs.getState().upsert(job);
  if (e.done) {
    metas.delete(e.id);
    if (status === 'done') {
      if (meta.kind === 'download') toast.success(t('devices.saved', { n: e.index }), meta.dest);
      else {
        void queryClient.invalidateQueries({ queryKey: ['browse'] });
        const tg = meta.target;
        toast.success(t('devices.uploaded', { n: e.index }), undefined, tg ? { label: t('jobs.open'), run: () => go(folderHref(tg.folderId, tg.dir)) } : undefined);
      }
    } else if (status === 'error') toast.error(t('devices.transferFailed'), e.error);
  }
  return job;
}

let listening: Promise<{ remove(): Promise<void> }> | null = null;

/** Start following native transfer events (idempotent). */
export function listenNativeTransfers() {
  if (!listening) listening = PeekLocal.addListener('transfer', (e) => applyTransferEvent(e));
  return listening;
}

function register(id: string, meta: Meta, count: number) {
  metas.set(id, meta);
  applyTransferEvent({ id, kind: meta.kind, file: '', index: 0, count, bytes: 0, total: 0, done: false });
}

export interface DownloadItem {
  url: string;
  name: string;
  mtime?: number | null;
  /** sub folder below the destination */
  dir?: string;
  size?: number;
}

/** Save server files permanently on the phone (native streaming download). */
export async function saveToPhone(backend: Backend, items: DownloadItem[], dest: string, title: string) {
  if (!(backend instanceof HttpBackend) || !items.length) return;
  await listenNativeTransfers();
  const id = `dl-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
  register(id, { kind: 'download', title, sizes: items.every((i) => typeof i.size === 'number') ? items.map((i) => i.size!) : null, createdAt: Date.now(), dest }, items.length);
  try {
    await PeekLocal.downloadTo({
      id,
      items: items.map((i) => ({ url: i.url, name: i.name, mtime: i.mtime ?? null, dir: i.dir || undefined })),
      dest,
      headers: backend.authHeaders(),
    });
  } catch (e) {
    applyTransferEvent({ id, kind: 'download', file: '', index: 0, count: items.length, bytes: 0, total: 0, done: true, error: (e as Error).message || 'error' });
  }
}

export function itemsForDownload(backend: Backend, items: MediaItem[], baseDir = '', prefix = ''): DownloadItem[] {
  return items.map((m) => {
    const rel = baseDir && m.dir.startsWith(baseDir) ? m.dir.slice(baseDir.length).replace(/^\/+/, '') : baseDir ? '' : m.dir;
    return { url: backend.fileUrl(m.id, true), name: m.name, mtime: m.mtime, size: m.size, dir: joinRel(prefix, rel) || undefined };
  });
}

/** Every media item below folderId/dir (recursive, all pages). */
export async function collectFolder(backend: Backend, folderId: string, dir: string): Promise<MediaItem[]> {
  const out: MediaItem[] = [];
  const LIMIT = 1000;
  for (let offset = 0; offset < 200_000; offset += LIMIT) {
    const r = await backend.browse({ folderId, dir, recursive: true, sort: 'name', order: 'asc', kind: 'all', offset, limit: LIMIT });
    out.push(...r.items);
    if (r.items.length < LIMIT || out.length >= r.total) break;
  }
  return out;
}

/** Upload phone files / folders into a server folder (native streaming upload, mtime kept). */
export async function uploadFromPhone(backend: Backend, paths: string[], target: { folderId: string; dir: string }, zip?: string) {
  if (!(backend instanceof HttpBackend) || !paths.length) return;
  await listenNativeTransfers();
  const id = `up-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
  const title = paths.length === 1 ? paths[0].slice(paths[0].lastIndexOf('/') + 1) : t('devices.nItems', { n: paths.length });
  register(id, { kind: 'upload', title, sizes: null, createdAt: Date.now(), target }, paths.length);
  try {
    await PeekLocal.uploadPaths({ id, url: backend.base || location.origin, folderId: target.folderId, dir: target.dir, paths, zip, headers: backend.authHeaders() });
  } catch (e) {
    applyTransferEvent({ id, kind: 'upload', file: '', index: 0, count: paths.length, bytes: 0, total: 0, done: true, error: (e as Error).message || 'error' });
  }
}

export async function cancelNative(jobId: string) {
  await PeekLocal.cancelTransfer({ id: jobId.slice(NATIVE_JOB.length) });
}
