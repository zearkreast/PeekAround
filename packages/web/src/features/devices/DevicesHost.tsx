import { useEffect, useRef } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { FileRef, Job, ServerEvent } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { isNative } from '@/backend/android/plugin';
import { useT } from '@/i18n';
import { go } from '@/lib/nav';
import { folderHref } from '@/lib/routes';
import { useJobs } from '@/store/jobs';
import { useSelection } from '@/store/selection';
import { toast } from '@/store/toasts';
import { FolderPicker } from '@/features/gallery/FolderPicker';
import { zipModeFor } from '@/features/upload/actions';
import { PairingDialog } from './PairingDialog';
import { PhoneFolderPicker } from './PhoneFolderPicker';
import { SendToPhoneDialog } from './SendToPhoneDialog';
import { collectFolder, itemsForDownload, listenNativeTransfers, saveToPhone, uploadFromPhone } from './transfers';
import { useCanApprove, useDeviceUi, useDevices } from './useDevices';
import './actions';

/** server transfer jobs this window started or saw running → toast when they finish */
const watched = new Set<string>();
const toasted = new Set<string>();

export function watchJob(j: Job | undefined | null) {
  if (!j) return;
  watched.add(j.id);
  useJobs.getState().upsert(j);
}

/**
 * Linked phones, app-wide: pairing requests (PC window), live device events, transfer toasts,
 * native phone transfers, and the dialogs the device actions open. Mount once (Layout).
 */
export function DevicesHost() {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const canApprove = useCanApprove();
  const devices = useDevices();
  const ui = useDeviceUi();
  const tRef = useRef(t);
  tRef.current = t;

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const refetch = () => {
      if (timer) return;
      timer = setTimeout(() => {
        timer = null;
        void qc.invalidateQueries({ queryKey: ['devices'] });
        void qc.invalidateQueries({ queryKey: ['libraries'] });
      }, 250);
    };
    const off = backend.subscribe((e: ServerEvent) => {
      if (e.type === 'devices') refetch();
      else if (e.type === 'pair') {
        useDeviceUi.getState().pushPair(e.device);
        refetch();
      } else if (e.type === 'job' && e.job.type === 'transfer') {
        const j = e.job;
        if (j.status === 'running' || j.status === 'queued') watched.add(j.id);
        else if (watched.has(j.id) && !toasted.has(j.id)) {
          toasted.add(j.id);
          const tt = tRef.current;
          if (j.status === 'done') {
            const r = j.result;
            const n = j.files?.total ?? 1;
            toast.success(tt('devices.transferDone', { n }), j.title, r ? { label: tt('jobs.open'), run: () => go(folderHref(r.folderId, r.dir)) } : undefined);
          } else if (j.status === 'error') toast.error(tt('devices.transferFailed'), j.error ?? undefined);
        }
      }
    });
    return () => {
      off();
      if (timer) clearTimeout(timer);
    };
  }, [backend, qc]);

  // pairing requests that are already pending when the window opens
  useEffect(() => {
    if (!canApprove) return;
    const st = useDeviceUi.getState();
    for (const d of devices.data ?? []) if (!d.paired && d.code && !st.dismissed.has(d.id) && !st.pairs.some((p) => p.id === d.id)) st.pushPair(d);
    // a request that disappeared (approved elsewhere, phone gave up) closes
    for (const p of st.pairs) if (devices.data && !devices.data.some((d) => d.id === p.id && !d.paired)) st.dropPair(p.id);
  }, [devices.data, canApprove]);

  // native phone transfers → jobs panel
  useEffect(() => {
    if (isNative()) void listenNativeTransfers().catch(() => {});
  }, []);

  const d = ui.dialog;
  const close = ui.close;

  const copyToPc = async (refs: FileRef[], target: { folderId: string; dir: string; label: string }) => {
    close();
    try {
      const r = await backend.move(refs, target, true);
      useSelection.getState().clear();
      const job = r && 'job' in r ? r.job : undefined;
      watchJob(job);
      toast.info(t('devices.copyStarted'), target.label);
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    }
  };

  return (
    <>
      <PairingDialog />
      {d?.kind === 'copyToPc' && (
        <FolderPicker open title={t('devices.copyToPcTitle', { what: d.label })} confirmLabel={t('devices.copyHere')} onClose={close} onPick={(tg) => void copyToPc(d.refs, tg)} />
      )}
      {d?.kind === 'sendToPhone' && <SendToPhoneDialog open label={d.label} onClose={close} onPick={(tg) => void copyToPc(d.refs, tg)} />}
      {(d?.kind === 'saveToPhone' || d?.kind === 'saveFolderToPhone') && (
        <PhoneFolderPicker
          open
          mode="folder"
          purpose="download"
          title={d.kind === 'saveToPhone' ? t('devices.saveTitle', { n: d.items.length }) : t('devices.saveFolderTitle', { name: d.name })}
          onClose={close}
          onPick={async ([dest]) => {
            close();
            try {
              if (d.kind === 'saveToPhone') {
                useSelection.getState().clear();
                const title = d.items.length === 1 ? d.items[0].name : t('devices.nItems', { n: d.items.length });
                await saveToPhone(backend, itemsForDownload(backend, d.items), dest, title);
              } else {
                toast.info(t('devices.collecting'));
                const all = await collectFolder(backend, d.folderId, d.dir);
                if (!all.length) {
                  toast.info(t('devices.emptyFolder'));
                  return;
                }
                await saveToPhone(backend, itemsForDownload(backend, all, d.dir, d.name), dest, d.name);
              }
            } catch (e) {
              toast.error(t('common.error'), (e as Error).message);
            }
          }}
        />
      )}
      {d?.kind === 'uploadFromPhone' && (
        <PhoneFolderPicker
          open
          mode="files"
          purpose="upload"
          title={t('devices.uploadTitle')}
          confirmLabel={t('devices.uploadConfirm')}
          onClose={close}
          onPick={async (paths) => {
            close();
            const zips = paths.filter((p) => /\.zip$/i.test(p)).map((p) => p.slice(p.lastIndexOf('/') + 1));
            const zip = zips.length ? await zipModeFor(backend, zips) : 'keep';
            if (!zip) return;
            await uploadFromPhone(backend, paths, d.target, zip);
          }}
        />
      )}
    </>
  );
}
