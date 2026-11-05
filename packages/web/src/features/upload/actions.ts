import { extOf, type Settings, type UploadZipMode } from '@peekaround/shared';
import type { Backend } from '@/backend';
import { desktop } from '@/backend/desktop';
import { translate } from '@/i18n';
import { go } from '@/lib/nav';
import { folderHref, libraryHref } from '@/lib/routes';
import { usePrefs } from '@/store/prefs';
import { toast, useToasts } from '@/store/toasts';
import { usePrompt, useUploads } from '@/store/uploads';
import { queryClient } from '@/lib/queryClient';
import { collectFromInput, type Collected, type Picked } from './collect';

const t = (k: Parameters<typeof translate>[1], v?: Record<string, string | number>) => translate(usePrefs.getState().lang, k, v);

type Target = { folderId: string; dir: string };

/** zip handling for these archive names: the setting, or ask (null = canceled). */
export async function zipModeFor(backend: Backend, names: string[]): Promise<UploadZipMode | null> {
  let settings: Settings | undefined;
  try {
    settings = await backend.settings();
  } catch {
    /* default to ask */
  }
  if (settings && settings.zipAction !== 'ask') return settings.zipAction;
  const r = await usePrompt.getState().ask<{ mode: UploadZipMode; remember: boolean } | null>({ kind: 'zip', names } as never);
  if (!r) return null;
  if (r.remember && r.mode !== 'keep') {
    await backend.updateSettings({ zipAction: r.mode }).catch(() => {});
    void queryClient.invalidateQueries({ queryKey: ['settings'] });
  }
  return r.mode;
}

async function pickTarget(): Promise<Target | null> {
  return usePrompt.getState().ask<Target | null>({ kind: 'target' } as never);
}

/** Main entry: files/folders were dropped or picked. */
export async function addFiles(backend: Backend, collected: Collected, target: Target | null, opts: { libraryId?: string | null } = {}) {
  const info = await backend.info().catch(() => null);
  if (info?.caps.readonly) {
    toast.error(t('drop.readonly'));
    return;
  }
  let files = collected.files;

  // Desktop: dropped folders can be linked into a library instead of copied.
  const linkable = collected.dirs.filter((d) => d.absPath);
  if (desktop && info?.caps.nativeFolders && linkable.length) {
    const choice = await usePrompt.getState().ask<'link' | 'copy' | null>({ kind: 'link', names: linkable.map((d) => d.name) } as never);
    if (!choice) return;
    if (choice === 'link') {
      const libs = await backend.libraries();
      const owner = opts.libraryId ?? libs.libraries.find((l) => l.folders.some((f) => f.id === target?.folderId))?.id;
      let goTo = '/';
      for (const d of linkable) {
        if (owner) {
          await backend.addFolder(owner, d.absPath!);
          goTo = libraryHref(owner);
        } else {
          const lib = await backend.createLibrary({ name: d.name, folders: [d.absPath!] });
          goTo = libraryHref(lib.id);
        }
      }
      void queryClient.invalidateQueries({ queryKey: ['libraries'] });
      toast.success(t('lib.created'));
      // the remaining loose files still get copied
      files = files.filter((f) => !f.fromDir);
      if (!files.length) {
        go(goTo);
        return;
      }
    } else if (target) {
      await backend.importLocal(linkable.map((d) => d.absPath!), target.folderId, target.dir);
      files = files.filter((f) => !f.fromDir);
      toast.success(t('upload.done'), t('upload.doneBody', { n: linkable.length }));
      if (!files.length) return;
    }
  }

  if (!files.length) return;
  if (!target) target = await pickTarget();
  if (!target) return;

  const zips = files.filter((f) => extOf(f.file.name) === 'zip');
  let mode: UploadZipMode = 'keep';
  if (zips.length) {
    const m = await zipModeFor(backend, zips.map((z) => z.file.name));
    if (!m) return;
    mode = m;
  }

  // Desktop: plain files with a real path are copied by the app, no upload needed.
  const local = desktop && info?.caps.nativeFolders ? files.filter((f) => f.absPath) : [];
  const remote = files.filter((f) => !local.includes(f));
  if (local.length) {
    const tid = useToasts.getState().push({ kind: 'progress', title: t('upload.title'), progress: 0.3 });
    try {
      const r = await backend.importLocal(local.map((f) => f.absPath!), target.folderId, target.dir, mode);
      useToasts.getState().update(tid, { kind: 'success', title: t('upload.done'), body: t('upload.doneBody', { n: local.length }) });
      afterZip(r, target);
    } catch (e) {
      useToasts.getState().update(tid, { kind: 'error', title: t('common.error'), body: (e as Error).message });
    }
  }
  if (remote.length) await runQueue(backend, remote, target, mode);
}

function afterZip(r: { tempFolder?: { id: string; name: string }; extractedDir?: string }, target: Target) {
  if (r.tempFolder) {
    toast.success(t('upload.tempReady'), r.tempFolder.name, { label: t('common.open'), run: () => go(folderHref(r.tempFolder!.id)) });
    go(folderHref(r.tempFolder.id));
  } else if (r.extractedDir) {
    toast.success(t('upload.extracted'), r.extractedDir, { label: t('common.open'), run: () => go(folderHref(target.folderId, r.extractedDir)) });
  }
}

const controllers = new Map<number, AbortController>();

export function cancelAllUploads() {
  for (const c of controllers.values()) c.abort();
}

async function runQueue(backend: Backend, files: Picked[], target: Target, mode: UploadZipMode) {
  const ids = useUploads.getState().add(files.map((f) => ({ name: f.relPath, size: f.file.size })));
  let i = 0;
  let ok = 0;
  let failed = 0;
  const worker = async () => {
    while (i < files.length) {
      const idx = i++;
      const f = files[idx];
      const id = ids[idx];
      const ctrl = new AbortController();
      controllers.set(id, ctrl);
      const patch = useUploads.getState().patch;
      patch(id, { status: 'uploading' });
      try {
        const isZip = extOf(f.file.name) === 'zip';
        const r = await backend.upload(f.file, {
          folderId: target.folderId,
          dir: target.dir,
          path: f.relPath,
          zip: isZip ? mode : undefined,
          // keep the original date (File.lastModified; 0 / missing → server time)
          mtime: f.file.lastModified > 0 ? f.file.lastModified : undefined,
          signal: ctrl.signal,
          onProgress: (loaded, total) => {
            patch(id, { loaded, status: loaded >= total && isZip && mode !== 'keep' ? 'processing' : 'uploading' });
          },
        });
        patch(id, { status: 'done', loaded: f.file.size });
        ok++;
        if (isZip) afterZip(r, target);
      } catch (e) {
        const canceled = ctrl.signal.aborted;
        patch(id, { status: canceled ? 'canceled' : 'error', error: (e as Error).message });
        if (!canceled) failed++;
      } finally {
        controllers.delete(id);
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(3, files.length) }, worker));
  void queryClient.invalidateQueries({ queryKey: ['browse'] });
  if (failed) toast.error(t('upload.failed', { n: failed }));
  else if (ok) toast.success(t('upload.done'), t('upload.doneBody', { n: ok }));
  setTimeout(() => {
    const s = useUploads.getState();
    if (s.entries.every((e) => e.status === 'done' || e.status === 'canceled')) {
      s.setOpen(false);
      setTimeout(() => useUploads.getState().clearDone(), 400);
    }
  }, 2500);
}

/** "Add files" / "Add folder" buttons. */
export function pickAndUpload(backend: Backend, target: Target, folder: boolean) {
  const input = document.createElement('input');
  input.type = 'file';
  input.multiple = true;
  if (folder) (input as HTMLInputElement & { webkitdirectory: boolean }).webkitdirectory = true;
  input.onchange = () => {
    const c = collectFromInput(input.files);
    if (desktop) for (const f of c.files) f.absPath = folder ? undefined : desktop.getPathForFile(f.file) || undefined;
    void addFiles(backend, c, target);
  };
  input.click();
}
