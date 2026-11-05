import { useEffect } from 'react';
import { useNavigate } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import { CheckCircle2, CircleAlert, CircleSlash, Film, Presentation, X } from 'lucide-react';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { folderHref } from '@/lib/routes';
import { useJobs } from '@/store/jobs';
import { Button, MenuLabel, ProgressBar } from '@/components/ui';

export function JobsPanel({ onNavigate }: { onNavigate?: () => void }) {
  const t = useT();
  const backend = useBackend();
  const nav = useNavigate();
  const { jobs, setAll } = useJobs();
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
              await backend.clearJobs();
              setAll(await backend.jobs());
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
              {j.type === 'slideshow' ? <Presentation className="size-4 shrink-0 text-accent" /> : <Film className="size-4 shrink-0 text-accent" />}
              <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{j.title}</span>
              {j.status === 'done' && <CheckCircle2 className="size-4 text-ok" />}
              {j.status === 'error' && <CircleAlert className="size-4 text-danger" />}
              {j.status === 'canceled' && <CircleSlash className="size-4 text-faint" />}
              {(j.status === 'running' || j.status === 'queued') && (
                <button aria-label={t('jobs.cancel')} className="text-faint hover:text-fg" onClick={() => backend.cancelJob(j.id)}>
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
                {t('jobs.open')} · <span className="truncate">{j.result.name}</span>
              </Button>
            )}
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
