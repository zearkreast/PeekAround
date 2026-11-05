import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useNavigate } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import { CircleAlert, CircleCheck, Film, FolderOpen, Presentation, X } from 'lucide-react';
import type { Job } from '@peekaround/shared';
import { useBackend, type Backend } from '@/backend';
import { useT } from '@/i18n';
import { folderHref, withView } from '@/lib/routes';
import { useJobs } from '@/store/jobs';
import { toast } from '@/store/toasts';
import { Button, IconButton, ProgressBar, ProgressRing } from '@/components/ui';

export type RunPhase = 'idle' | 'preparing' | 'job' | 'error';

/** Find the media id of a freshly written file (to open it in the viewer). */
export async function findMediaId(backend: Backend, r: NonNullable<Job['result']>): Promise<string | null> {
  try {
    const res = await backend.browse({ folderId: r.folderId, dir: r.dir, sort: 'mtime', order: 'desc', limit: 500 });
    return res.items.find((i) => i.name === r.name)?.id ?? null;
  } catch {
    return null;
  }
}

export function resultHref(r: NonNullable<Job['result']>, id: string | null): string {
  return `${folderHref(r.folderId, r.dir)}${withView('', id)}`;
}

const RUN_KEYS = {
  video: { started: 'video.exportStarted', done: 'video.exportDone', failed: 'video.exportFailed' },
  slideshow: { started: 'slide.exportStarted', done: 'slide.exportDone', failed: 'slide.exportFailed' },
} as const;

/**
 * Runs "prepare → start job → watch job" and reports through toasts.
 * `prepare` gets a progress callback for the client-side part (e.g. rendering overlays).
 */
export function useJobRun(kind: 'video' | 'slideshow') {
  const t = useT();
  const k = RUN_KEYS[kind];
  const backend = useBackend();
  const nav = useNavigate();
  const [phase, setPhase] = useState<RunPhase>('idle');
  const [prep, setPrep] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [jobId, setJobId] = useState<string | null>(null);
  const [resultId, setResultId] = useState<string | null>(null);
  const job = useJobs((s) => (jobId ? s.jobs[jobId] : undefined));
  const announced = useRef<string | null>(null);

  const open = useCallback(
    (r: NonNullable<Job['result']>, id: string | null) => {
      nav(resultHref(r, id));
    },
    [nav],
  );

  useEffect(() => {
    if (!job || announced.current === `${job.id}:${job.status}`) return;
    if (job.status === 'done' && job.result) {
      announced.current = `${job.id}:${job.status}`;
      const r = job.result;
      void findMediaId(backend, r).then((id) => {
        setResultId(id);
        toast.success(t(k.done), r.name, { label: t('jobs.open'), run: () => open(r, id) });
      });
    } else if (job.status === 'error') {
      announced.current = `${job.id}:${job.status}`;
      toast.error(t(k.failed), job.error ?? undefined);
    }
  }, [job, backend, t, open, k]);

  const start = useCallback(
    async (run: (onPrep: (p: number) => void) => Promise<Job>) => {
      setPhase('preparing');
      setPrep(0);
      setError(null);
      setResultId(null);
      setJobId(null);
      try {
        const j = await run(setPrep);
        useJobs.getState().upsert(j);
        setJobId(j.id);
        setPhase('job');
        toast.info(t(k.started), j.title);
      } catch (e) {
        setPhase('error');
        setError((e as Error).message);
        toast.error(t(k.failed), (e as Error).message);
      }
    },
    [t, k],
  );

  const close = useCallback(() => {
    setPhase('idle');
    setJobId(null);
  }, []);

  const busy = phase === 'preparing' || (phase === 'job' && (!job || job.status === 'queued' || job.status === 'running'));
  return { phase, prep, error, job, jobId, resultId, busy, start, close, open };
}

/** Floating progress card for an export job. */
export function JobProgressCard({
  run,
  kind,
  title,
  extra,
}: {
  run: ReturnType<typeof useJobRun>;
  kind: 'video' | 'slideshow';
  title: string;
  extra?: ReactNode;
}) {
  const t = useT();
  const backend = useBackend();
  const { phase, job, prep, error } = run;
  const visible = phase !== 'idle';
  const status = phase === 'preparing' ? 'preparing' : phase === 'error' ? 'error' : (job?.status ?? 'queued');
  const progress = phase === 'preparing' ? prep : (job?.progress ?? 0);
  const done = status === 'done';
  const failed = status === 'error' || status === 'canceled';
  const label =
    status === 'preparing'
      ? t('video.preparing')
      : status === 'queued'
        ? t('jobs.queued')
        : status === 'running'
          ? t('jobs.running')
          : status === 'done'
            ? t('jobs.done')
            : status === 'canceled'
              ? t('jobs.canceled')
              : t('jobs.error');

  // portal: `fixed` must not be trapped by transformed (animated) ancestors
  return createPortal(
    <AnimatePresence>
      {visible && (
        <motion.div
          initial={{ opacity: 0, y: 40, scale: 0.94 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 24, scale: 0.96 }}
          transition={{ type: 'spring', stiffness: 380, damping: 30 }}
          className="glass fixed right-4 bottom-4 z-[70] w-[min(360px,calc(100vw-2rem))] overflow-hidden rounded-3xl border border-line p-4 shadow-soft"
          role="status"
        >
          {done && <motion.div className="pointer-events-none absolute inset-0 bg-accent-gradient" initial={{ opacity: 0.35 }} animate={{ opacity: 0 }} transition={{ duration: 1.2 }} />}
          <div className="relative flex items-center gap-3">
            <div className="relative grid size-11 shrink-0 place-items-center">
              <AnimatePresence mode="popLayout" initial={false}>
                {done ? (
                  <motion.span key="ok" initial={{ scale: 0, rotate: -45 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: 'spring', stiffness: 500, damping: 18 }}>
                    <CircleCheck className="size-9 text-ok" />
                  </motion.span>
                ) : failed ? (
                  <motion.span key="err" initial={{ scale: 0 }} animate={{ scale: 1 }}>
                    <CircleAlert className="size-9 text-danger" />
                  </motion.span>
                ) : (
                  <motion.span key="ring" className="relative grid place-items-center" exit={{ scale: 0.6, opacity: 0 }}>
                    <ProgressRing value={progress} size={44} stroke={4} />
                    <span className="absolute text-accent [&>svg]:size-4">{kind === 'slideshow' ? <Presentation /> : <Film />}</span>
                  </motion.span>
                )}
              </AnimatePresence>
            </div>
            <div className="min-w-0 flex-1">
              <div className="truncate text-sm font-semibold">{done && job?.result ? job.result.name : title}</div>
              <div className="text-xs text-dim tabular-nums">
                {label}
                {!done && !failed && ` · ${Math.round(progress * 100)}%`}
              </div>
            </div>
            {!run.busy && (
              <IconButton label={t('common.close')} size="sm" onClick={run.close}>
                <X />
              </IconButton>
            )}
          </div>
          {!done && !failed && <ProgressBar value={progress} className="relative mt-3" />}
          {failed && (error || job?.error) && <p className="relative mt-2 line-clamp-3 text-xs text-danger">{error || job?.error}</p>}
          <div className="relative mt-3 flex flex-wrap items-center justify-end gap-2">
            {run.busy && job && (
              <Button size="sm" variant="ghost" onClick={() => void backend.cancelJob(job.id)}>
                {t('jobs.cancel')}
              </Button>
            )}
            {done && job?.result && (
              <>
                {extra}
                <Button size="sm" variant="primary" icon={<FolderOpen className="size-4" />} onClick={() => run.open(job.result!, run.resultId)}>
                  {t('jobs.open')}
                </Button>
              </>
            )}
          </div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}
