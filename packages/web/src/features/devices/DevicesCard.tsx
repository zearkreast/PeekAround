import { useState } from 'react';
import { Link } from 'react-router';
import { motion } from 'motion/react';
import { useQueryClient } from '@tanstack/react-query';
import { Check, FolderOpen, Smartphone, Unlink, X } from 'lucide-react';
import type { DeviceInfo } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { isNative } from '@/backend/android/plugin';
import { useLang, useT } from '@/i18n';
import { folderHref } from '@/lib/routes';
import { toast } from '@/store/toasts';
import { Badge, Button, ConfirmDialog } from '@/components/ui';
import { deviceStatus, OnlineDot } from './DevicesNav';
import { useCanApprove, useDevices } from './useDevices';

/** Settings → "Cihazlar": linked phones (pending requests, paired devices, their folders, forget). */
export function DevicesCard() {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const devices = useDevices();
  const canApprove = useCanApprove();
  const [forget, setForget] = useState<DeviceInfo | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  if (!backend.devices || !info || info.mode === 'android-local') return null;
  const list = devices.data ?? [];
  // a phone browsing the PC (client mode) only sees the other phones; nothing to manage there
  if (isNative() && !list.length) return null;

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['devices'] });
    void qc.invalidateQueries({ queryKey: ['libraries'] });
  };
  const pair = async (d: DeviceInfo, approve: boolean) => {
    setBusy(d.id);
    try {
      await backend.pairDevice?.(d.id, approve);
      refresh();
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const pending = list.filter((d) => !d.paired);
  const paired = list.filter((d) => d.paired);

  return (
    <motion.section
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: 'spring', stiffness: 260, damping: 28, delay: 0.1 }}
      className="rounded-3xl border border-line bg-elev p-5 sm:p-6"
      data-testid="devices-card"
    >
      <h2 className="mb-1 flex items-center gap-2.5 font-display text-lg font-semibold">
        <span className="grid size-8 place-items-center rounded-xl bg-surface-2 text-accent">
          <Smartphone className="size-4" />
        </span>
        {t('devices.title')}
      </h2>
      <p className="mb-4 text-xs text-faint">{info.mode === 'desktop' ? t('devices.cardHintDesktop') : t('devices.cardHint')}</p>

      {!list.length && <p className="rounded-2xl bg-surface p-4 text-sm text-dim">{t('devices.empty')}</p>}

      <div className="flex flex-col divide-y divide-line">
        {pending.map((d) => (
          <div key={d.id} className="flex flex-wrap items-center gap-3 py-3 first:pt-0">
            <span className="grid size-10 place-items-center rounded-2xl bg-[color-mix(in_oklab,var(--warn)_18%,transparent)] text-warn">
              <Smartphone className="size-5" />
            </span>
            <div className="min-w-0 flex-1">
              <div className="truncate text-sm font-medium">{d.name}</div>
              <div className="text-xs text-faint">
                {t('devices.pendingCode')} <span className="font-mono text-sm font-semibold text-fg tracking-widest">{d.code}</span>
              </div>
            </div>
            {canApprove && (
              <div className="flex gap-1.5">
                <Button size="sm" variant="ghost" icon={<X className="size-4" />} disabled={busy === d.id} onClick={() => pair(d, false)}>
                  {t('devices.pair.reject')}
                </Button>
                <Button size="sm" variant="primary" icon={<Check className="size-4" />} loading={busy === d.id} onClick={() => pair(d, true)}>
                  {t('devices.pair.approve')}
                </Button>
              </div>
            )}
          </div>
        ))}
        {paired.map((d) => (
          <div key={d.id} className="flex flex-col gap-2 py-3.5 first:pt-0 last:pb-0">
            <div className="flex items-center gap-3">
              <span className="relative grid size-10 shrink-0 place-items-center rounded-2xl bg-surface-2 text-dim">
                <Smartphone className="size-5" />
                <OnlineDot online={d.online} className="absolute right-0.5 bottom-0.5 ring-2 ring-[var(--bg-elev)]" />
              </span>
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium">{d.name}</div>
                <div className="truncate text-xs text-faint">
                  {[d.model !== d.name ? d.model : '', deviceStatus(d, t, lang)].filter(Boolean).join(' · ')}
                </div>
              </div>
              {canApprove && (
                <Button size="sm" variant="ghost" icon={<Unlink className="size-4" />} onClick={() => setForget(d)}>
                  {t('devices.forget')}
                </Button>
              )}
            </div>
            {d.folders.length > 0 && (
              <div className="flex flex-wrap gap-1.5 pl-[52px]">
                {d.folders.map((f) => (
                  <Link key={f.id} to={folderHref(f.id)} className="inline-flex items-center gap-1.5 rounded-full border border-line bg-surface px-2.5 py-1 text-xs text-dim hover:text-fg">
                    <FolderOpen className="size-3.5" />
                    {f.name}
                    <Badge className="!px-1.5 !py-0 text-[10px]">{f.mediaCount}</Badge>
                  </Link>
                ))}
              </div>
            )}
          </div>
        ))}
      </div>

      <ConfirmDialog
        open={!!forget}
        danger
        title={t('devices.forgetTitle', { name: forget?.name ?? '' })}
        body={t('devices.forgetBody')}
        confirmLabel={t('devices.forget')}
        cancelLabel={t('common.cancel')}
        onClose={() => setForget(null)}
        onConfirm={async () => {
          if (!forget) return;
          await backend.forgetDevice?.(forget.id);
          refresh();
        }}
      />
    </motion.section>
  );
}
