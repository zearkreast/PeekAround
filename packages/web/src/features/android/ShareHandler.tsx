import { useCallback, useEffect, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useBackend } from '@/backend';
import { HttpBackend } from '@/backend/http';
import { PeekLocal, isNative, type SharedFile } from '@/backend/android/plugin';
import { useLang } from '@/i18n';
import { toast, useToasts } from '@/store/toasts';
import { FolderPicker } from '@/features/gallery/FolderPicker';
import { L, fmt } from './strings';

/** Android: files sent via "Share → PeekAround" are uploaded/copied into a folder the user picks. */
export function ShareHandler() {
  const backend = useBackend();
  const lang = useLang();
  const s = L[lang];
  const qc = useQueryClient();
  const [files, setFiles] = useState<SharedFile[]>([]);

  const check = useCallback(async () => {
    try {
      const r = await PeekLocal.getShared();
      if (r.files.length) setFiles(r.files);
    } catch {
      /* not native */
    }
  }, []);

  useEffect(() => {
    if (!isNative()) return;
    void check();
    const h = PeekLocal.addListener('shared', () => void check());
    return () => {
      void h.then((x) => x.remove());
    };
  }, [check]);

  if (!files.length) return null;

  return (
    <FolderPicker
      open
      title={`${fmt(s.sharedTitle, { n: files.length })} — ${s.sharedBody}`}
      onClose={() => {
        setFiles([]);
        void PeekLocal.clearShared();
      }}
      onPick={async (target) => {
        const list = files;
        setFiles([]);
        const tid = useToasts.getState().push({ kind: 'progress', title: fmt(s.uploading, { i: 0, n: list.length }), progress: 0 });
        try {
          if (backend instanceof HttpBackend) {
            const h = await PeekLocal.addListener('uploadProgress', (e) => {
              const p = (e.index + (e.size > 0 ? e.sent / e.size : 0)) / e.total;
              useToasts.getState().update(tid, { title: fmt(s.uploading, { i: e.index + 1, n: e.total }), progress: p });
            });
            await PeekLocal.uploadShared({ url: backend.base || location.origin, folderId: target.folderId, dir: target.dir, files: list });
            await h.remove();
          } else {
            const libs = await backend.libraries();
            const f = [...libs.libraries.flatMap((l) => l.folders), ...libs.temp].find((x) => x.id === target.folderId);
            if (!f) throw new Error('Folder not found');
            await PeekLocal.importShared({ dir: [f.path, target.dir].filter(Boolean).join('/'), files: list });
          }
          useToasts.getState().update(tid, { kind: 'success', title: fmt(s.sharedDone, { n: list.length }) });
          await qc.invalidateQueries({ queryKey: ['browse'] });
        } catch (e) {
          useToasts.getState().update(tid, { kind: 'error', title: (e as Error).message });
          toast.error((e as Error).message);
        }
      }}
    />
  );
}
