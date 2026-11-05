import { useEffect } from 'react';
import { useNavigate } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import { ArrowDownUp, CheckCircle2, CircleAlert, CircleSlash, Film, Presentation, X } from 'lucide-react';
import type { Job } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { folderHref } from '@/lib/routes';
import { etaOf, isNativeJob, useJobs } from '@/store/jobs';
import { formatBytes } from '@/lib/format';
import { cancelNative } from '@/features/devices/transfers';
import { Button, MenuLabel, ProgressBar } from '@/components/ui';

export function JobsPanel({ onNavigate }: { onNavigate?: () => void }) {
  const t = useT();
  const backend = useBackend();
  const nav = useNavigate();
  const { jobs, setAll, rates, clearNative } = useJobs();
  useEffect(() => {
    backend.jobs().then(setAll).catch(() => {});
  }, [backend, setAll]);
  const list = Object.values(jobs).sort((a, b) => b.createdAt - a.createdAt);
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between">
        <MenuLabel>{t('jobs.title')}</MenuLabel>
        {list.some((j) => j.status !== 'running' && j.status !== 'queued') && (
          <button
            className="px-2 text-xs text-faint hover:text-fg"
            onClick={async () => {
              clearNative();
              await backend.clearJobs().catch(() => {});
              setAll(await backend.jobs().catch(() => []));
            }}
          >
            {t('jobs.clear')}
          </button>
        )}
      </div>
      <AnimatePresence initial={false}>
        {list.map((j) => (
          <motion.div key={j.id} layout initial={{ opacity: 0, y: -6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="rounded-xl bg-surface p-3">
            <div className="flex items-center gap-2.5">
              {j.type === 'slideshow' ? (
                <Presentation className="size-4 shrink-0 text-accent" />
              ) : j.type === 'transfer' ? (
                <ArrowDownUp className="size-4 shrink-0 text-accent" />
              ) : (
                <Film className="size-4 shrink-0 text-accent" />
              )}
              <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{j.title}</span>
              {j.status === 'done' && <CheckCircle2 className="size-4 text-ok" />}
              {j.status === 'error' && <CircleAlert className="size-4 text-danger" />}
              {j.status === 'canceled' && <CircleSlash className="size-4 text-faint" />}
              {(j.status === 'running' || j.status === 'queued') && (
                <button aria-label={t('jobs.cancel')} className="text-faint hover:text-fg" onClick={() => void (isNativeJob(j.id) ? cancelNative(j.id) : backend.cancelJob(j.id)).catch(() => {})}>
                  <X className="size-4" />
                </button>
              )}
            </div>
            {(j.status === 'running' || j.status === 'queued') && (
              <div className="mt-2 flex items-center gap-2">
                <ProgressBar value={j.progress} />
                <span className="w-9 text-right text-[11px] text-faint tabular-nums">{j.status === 'queued' ? '…' : `${Math.round(j.progress * 100)}%`}</span>
              </div>
            )}
            {j.type === 'transfer' && (j.bytes || j.files) && j.status !== 'canceled' && <TransferLine j={j} rate={rates[j.id]} />}
            {j.status === 'error' && j.error && <p className="mt-1.5 line-clamp-2 text-xs text-danger">{j.error}</p>}
            {j.status === 'done' && j.result && (
              <Button
                size="sm"
                variant="ghost"
                className="mt-1 -ml-2"
                onClick={() => {
                  nav(folderHref(j.result!.folderId, j.result!.dir));
                  onNavigate?.();
                }}
              >
                {t('jobs.open')}
                {j.result.name && (
                  <>
                    {' · '}
                    <span className="truncate">{j.result.name}</span>
                  </>
                )}
              </Button>
            )}
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}

/** "3/12 files · 40 MB / 120 MB · 5.2 MB/s · 1 min left" */
function TransferLine({ j, rate }: { j: Job; rate: number | undefined }) {
  const t = useT();
  const parts: string[] = [];
  if (j.files && j.files.total > 1) parts.push(t('devices.jobFiles', { done: j.files.done, total: j.files.total }));
  if (j.bytes && j.bytes.total > 0) parts.push(j.status === 'done' ? formatBytes(j.bytes.total) : `${formatBytes(j.bytes.done)} / ${formatBytes(j.bytes.total)}`);
  if (j.status === 'running' && rate && rate > 1) parts.push(`${formatBytes(rate)}/s`);
  const eta = etaOf(j, rate);
  if (eta != null && eta > 2) parts.push(eta < 60 ? t('devices.etaSec', { n: Math.ceil(eta) }) : t('devices.etaMin', { n: Math.ceil(eta / 60) }));
  if (!parts.length) return null;
  return <div className="mt-1 truncate text-[11px] text-faint tabular-nums">{parts.join(' · ')}</div>;
}
