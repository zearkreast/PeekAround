import { AnimatePresence, motion } from 'motion/react';
import { CheckCircle2, AlertCircle, Info, X } from 'lucide-react';
import { useToasts } from '@/store/toasts';
import { ProgressRing } from './ui';

export function Toaster() {
  const { toasts, dismiss } = useToasts();
  return (
    <div className="pointer-events-none fixed bottom-5 left-1/2 z-[100] flex w-[min(420px,calc(100vw-32px))] -translate-x-1/2 flex-col items-stretch gap-2">
      <AnimatePresence initial={false}>
        {toasts.map((t) => (
          <motion.div
            key={t.id}
            layout
            initial={{ opacity: 0, y: 30, scale: 0.9 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 10, scale: 0.95, transition: { duration: 0.15 } }}
            transition={{ type: 'spring', stiffness: 500, damping: 34 }}
            className="glass pointer-events-auto flex items-start gap-3 rounded-2xl border border-line px-4 py-3 shadow-soft"
          >
            <span className="mt-0.5 shrink-0">
              {t.kind === 'success' && <CheckCircle2 className="size-5 text-ok" />}
              {t.kind === 'error' && <AlertCircle className="size-5 text-danger" />}
              {t.kind === 'info' && <Info className="size-5 text-accent" />}
              {t.kind === 'progress' && <ProgressRing value={t.progress ?? 0} size={20} stroke={2.5} />}
            </span>
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium text-fg">{t.title}</div>
              {t.body && <div className="mt-0.5 truncate text-[13px] text-dim">{t.body}</div>}
            </div>
            {t.action && (
              <button
                className="shrink-0 rounded-lg px-2 py-1 text-[13px] font-semibold text-accent hover:bg-surface-2"
                onClick={() => {
                  t.action!.run();
                  dismiss(t.id);
                }}
              >
                {t.action.label}
              </button>
            )}
            <button className="shrink-0 text-faint hover:text-fg" onClick={() => dismiss(t.id)} aria-label="Close">
              <X className="size-4" />
            </button>
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
