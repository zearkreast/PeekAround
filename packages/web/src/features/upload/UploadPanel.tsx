import { AnimatePresence, motion } from 'motion/react';
import { CheckCircle2, ChevronDown, CircleAlert, FileUp, Loader2, X } from 'lucide-react';
import { useState } from 'react';
import { useT } from '@/i18n';
import { formatBytes } from '@/lib/format';
import { useUploads } from '@/store/uploads';
import { ProgressBar, ProgressRing } from '@/components/ui';
import { cancelAllUploads } from './actions';

export function UploadPanel() {
  const t = useT();
  const { entries, open, setOpen } = useUploads();
  const [expanded, setExpanded] = useState(true);
  const total = entries.reduce((a, e) => a + e.size, 0);
  const loaded = entries.reduce((a, e) => a + (e.status === 'done' ? e.size : e.loaded), 0);
  const done = entries.filter((e) => e.status === 'done').length;
  const active = entries.some((e) => e.status === 'uploading' || e.status === 'queued' || e.status === 'processing');
  return (
    <AnimatePresence>
      {open && entries.length > 0 && (
        <motion.div
          initial={{ opacity: 0, y: 40, scale: 0.95 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 30, scale: 0.95 }}
          transition={{ type: 'spring', stiffness: 400, damping: 32 }}
          className="glass fixed right-5 bottom-5 z-[60] w-[340px] overflow-hidden rounded-2xl border border-line shadow-soft"
        >
          <div className="flex items-center gap-3 px-4 py-3">
            <ProgressRing value={total ? loaded / total : 0} size={26} />
            <div className="min-w-0 flex-1">
              <div className="text-sm font-semibold">{t('upload.title')}</div>
              <div className="text-xs text-dim tabular-nums">
                {t('upload.progress', { done, total: entries.length })} · {formatBytes(loaded)} / {formatBytes(total)}
              </div>
            </div>
            <button className="grid size-7 place-items-center rounded-lg text-faint hover:bg-surface-2 hover:text-fg" onClick={() => setExpanded((x) => !x)} aria-label="Toggle">
              <ChevronDown className={`size-4 transition-transform ${expanded ? '' : 'rotate-180'}`} />
            </button>
            <button
              className="grid size-7 place-items-center rounded-lg text-faint hover:bg-surface-2 hover:text-fg"
              title={active ? t('upload.cancelAll') : t('common.close')}
              onClick={() => (active ? cancelAllUploads() : setOpen(false))}
            >
              <X className="size-4" />
            </button>
          </div>
          <AnimatePresence initial={false}>
            {expanded && (
              <motion.div initial={{ height: 0 }} animate={{ height: 'auto' }} exit={{ height: 0 }} className="overflow-hidden">
                <div className="max-h-64 overflow-auto border-t border-line px-2 py-2">
                  {entries.map((e) => (
                    <div key={e.id} className="flex items-center gap-2.5 rounded-xl px-2 py-1.5">
                      {e.status === 'done' ? (
                        <CheckCircle2 className="size-4 shrink-0 text-ok" />
                      ) : e.status === 'error' ? (
                        <CircleAlert className="size-4 shrink-0 text-danger" />
                      ) : e.status === 'processing' ? (
                        <Loader2 className="size-4 shrink-0 animate-spin text-accent" />
                      ) : (
                        <FileUp className="size-4 shrink-0 text-faint" />
                      )}
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-[13px]" title={e.error || e.name}>
                          {e.name}
                        </div>
                        {(e.status === 'uploading' || e.status === 'queued') && <ProgressBar value={e.size ? e.loaded / e.size : 0} className="mt-1 h-1" />}
                        {e.status === 'processing' && <div className="text-[11px] text-accent">{t('upload.extracting')}</div>}
                        {e.status === 'error' && <div className="truncate text-[11px] text-danger">{e.error}</div>}
                      </div>
                      <span className="shrink-0 text-[11px] text-faint tabular-nums">{formatBytes(e.size)}</span>
                    </div>
                  ))}
                </div>
              </motion.div>
            )}
          </AnimatePresence>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
