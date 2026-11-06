/**
 * Remote (streaming) MEGA folders in the gallery: status badge (sidebar), banner (folder / library
 * page) with "Yenile" / "Bilgisayara kopyala" / "Bağlantıyı kaldır", and the "copy to this computer"
 * media action. The link key never reaches the UI except through the local-only remoteLink().
 */
import { useState } from 'react';
import { useNavigate } from 'react-router';
import { motion } from 'motion/react';
import { useQueryClient } from '@tanstack/react-query';
import { Cloud, CloudOff, HardDriveDownload, Link2, RefreshCw, TriangleAlert, Unlink } from 'lucide-react';
import type { FolderRef, Library, MediaItem, RemoteFolderInfo } from '@peekaround/shared';
import { ApiError, useBackend, useInfo } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { useLibraries } from '@/lib/queries';
import { toast } from '@/store/toasts';
import { Button, ConfirmDialog, ProgressRing, Spinner } from '@/components/ui';
import { isPhoneClient, refOf, registerMediaAction, type ActionCtx } from '@/features/gallery/actions';
import { useDeviceUi } from '@/features/devices/useDevices';
import { megaErrorKey } from './mega';

import { remoteStatusText } from './status';

export { remoteStatusText };

const tone = (r: RemoteFolderInfo) => (r.state === 'error' ? 'text-danger' : r.state === 'quota' || r.state === 'offline' ? 'text-warn' : 'text-[#e5484d]');

/** small cloud mark (sidebar library rows, folder cards); spins while listing, warns on problems */
export function RemoteBadge({ remote, className }: { remote: RemoteFolderInfo; className?: string }) {
  const t = useT();
  const lang = useLang();
  const busy = remote.state === 'listing' || (remote.state === 'ok' && remote.indexing.done < remote.indexing.total);
  const Icon = remote.state === 'offline' || remote.state === 'error' ? CloudOff : Cloud;
  return (
    <span className={cn('inline-flex shrink-0 items-center gap-1', tone(remote), className)} title={`${t('mega.readonly')} · ${remoteStatusText(remote, t, lang)}`} data-testid="remote-badge">
      <Icon className="size-3.5" />
      {busy && <span className="size-1.5 animate-pulse rounded-full bg-current" />}
    </span>
  );
}

export const remoteFoldersOf = (lib: Library | undefined): FolderRef[] => (lib?.folders ?? []).filter((f) => !!f.remote);

function RemoteRow({ f, dir }: { f: FolderRef; dir: string }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const nav = useNavigate();
  const openDevUi = useDeviceUi((s) => s.open);
  const [refreshing, setRefreshing] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const r = f.remote!;
  const writable = !!info && !info.caps.readonly;
  const pct = r.indexing.total ? r.indexing.done / r.indexing.total : 1;
  const problem = r.state === 'error' || r.state === 'offline' || r.state === 'quota';

  const refresh = async () => {
    setRefreshing(true);
    try {
      await backend.remoteRefresh!(f.id);
      toast.success(t('mega.refreshed'));
    } catch (e) {
      const err = e as ApiError;
      const m = megaErrorKey(err.status ?? 0, err.message ?? '');
      toast.error(t('common.error'), t(m.key, m.vars));
    } finally {
      setRefreshing(false);
      void qc.invalidateQueries({ queryKey: ['libraries'] });
      void qc.invalidateQueries({ queryKey: ['browse'] });
    }
  };

  const copyLink = async () => {
    try {
      const url = await backend.remoteLink!(f.id);
      await navigator.clipboard.writeText(url);
      toast.success(t('mega.linkCopied'));
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    }
  };

  return (
    <div
      className={cn(
        'flex flex-wrap items-center gap-3 rounded-2xl border px-4 py-3',
        problem ? 'border-[color-mix(in_oklab,var(--warn)_35%,transparent)] bg-[color-mix(in_oklab,var(--warn)_9%,transparent)]' : 'border-line bg-surface',
      )}
      data-testid="remote-banner"
    >
      <span className="grid size-9 shrink-0 place-items-center rounded-xl bg-[#d9272e]/15 text-[#e5484d]">
        {r.state === 'error' ? <TriangleAlert className="size-4 text-danger" /> : r.state === 'offline' ? <CloudOff className="size-4" /> : <Cloud className="size-4" />}
      </span>
      <div className="min-w-[min(calc(100%-3rem),16rem)] flex-1">
        <div className="flex items-center gap-2 text-sm font-medium">
          <span className="truncate">{f.name}</span>
          <span className="shrink-0 rounded-full bg-[#d9272e]/15 px-1.5 py-px text-[10px] font-semibold tracking-wide text-[#e5484d]">{t('mega.badge')}</span>
        </div>
        <div className={cn('flex items-center gap-1.5 text-xs', problem ? tone(r) : 'text-dim')}>
          {r.state === 'listing' ? <Spinner className="size-3" /> : r.indexing.done < r.indexing.total && r.state === 'ok' ? <ProgressRing value={pct} size={12} stroke={2} /> : null}
          <span className="truncate">{remoteStatusText(r, t, lang)}</span>
          <span className="hidden text-faint sm:inline">· {t('mega.readonly')}</span>
        </div>
      </div>
      <Button size="sm" variant="ghost" icon={<RefreshCw className={cn('size-4', refreshing && 'animate-spin')} />} disabled={refreshing || r.state === 'listing'} onClick={() => void refresh()}>
        {t('mega.refresh')}
      </Button>
      {writable && !isPhoneClient({ backend }) && (
        <Button
          size="sm"
          variant="secondary"
          icon={<HardDriveDownload className="size-4" />}
          aria-label={t('mega.copyToComputer')}
          onClick={() => openDevUi({ kind: 'copyToPc', refs: [{ folderId: f.id, rel: dir }], label: dir ? dir.split('/').pop()! : f.name })}
        >
          <span className="hidden sm:inline">{t('mega.copyToComputer')}</span>
        </Button>
      )}
      {info?.mode === 'desktop' && (
        <Button size="sm" variant="ghost" icon={<Link2 className="size-4" />} aria-label={t('mega.copyLink')} title={t('mega.copyLink')} onClick={() => void copyLink()} />
      )}
      {writable && (
        <Button size="sm" variant="ghost" icon={<Unlink className="size-4" />} aria-label={t('mega.remove')} title={t('mega.remove')} onClick={() => setConfirm(true)} />
      )}
      <ConfirmDialog
        open={confirm}
        danger
        title={t('mega.removeConfirm', { name: f.name })}
        body={t('mega.removeBody')}
        confirmLabel={t('mega.remove')}
        cancelLabel={t('common.cancel')}
        onClose={() => setConfirm(false)}
        onConfirm={async () => {
          try {
            await backend.removeFolder(f.id);
            await qc.invalidateQueries({ queryKey: ['libraries'] });
            toast.success(t('mega.removed'));
            nav('/');
          } catch (e) {
            toast.error(t('common.error'), (e as Error).message);
          }
        }}
      />
    </div>
  );
}

/** status + actions of the remote folder(s) on a folder or library page */
export function RemoteBanner({ folderId, libraryId, dir = '' }: { folderId?: string; libraryId?: string; dir?: string }) {
  const libs = useLibraries();
  const all = libs.data?.libraries ?? [];
  const list = folderId ? all.flatMap((l) => l.folders).filter((f) => f.id === folderId && !!f.remote) : remoteFoldersOf(all.find((l) => l.id === libraryId));
  if (!list.length) return null;
  return (
    <motion.div initial={{ opacity: 0, y: -10 }} animate={{ opacity: 1, y: 0 }} className="mb-6 flex flex-col gap-2">
      {list.map((f) => (
        <RemoteRow key={f.id} f={f} dir={folderId ? dir : ''} />
      ))}
    </motion.div>
  );
}

// ── media action: copy MEGA items to this computer (the import job, nothing re-listed) ──

const label = (items: MediaItem[], c: ActionCtx) => (items.length === 1 ? items[0].name : c.t('mega.nItems', { n: items.length }));

registerMediaAction({
  id: 'copy-to-computer',
  icon: <HardDriveDownload />,
  labelKey: 'mega.copyToComputer',
  shortLabelKey: 'mega.copyToComputerShort',
  group: 'primary',
  order: 12,
  primary: true,
  clearsSelection: false,
  visible: (items, c) =>
    c.writable && c.backend.kind === 'http' && !isPhoneClient(c) && items.length > 0 && !!c.remoteOf && items.every((i) => !!c.remoteOf!(i.folderId)) && new Set(items.map((i) => i.folderId)).size === 1,
  run: (items, c) => useDeviceUi.getState().open({ kind: 'copyToPc', refs: items.map(refOf), label: label(items, c) }),
});
