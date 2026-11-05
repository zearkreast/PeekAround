import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { AnimatePresence, motion, useDragControls } from 'motion/react';
import { cn } from '@/lib/cn';
import { BACK, useBackHandler } from '@/lib/backStack';
import { useDialogFocus } from '@/lib/useDialogFocus';

/**
 * Mobile bottom sheet: slides up, drag the handle (or header) down to dismiss,
 * closes on scrim tap / Escape / system back. Content scrolls inside.
 */
export function BottomSheet({
  open,
  onClose,
  title,
  children,
  className,
  dark,
  z = 85,
}: {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  children: ReactNode;
  className?: string;
  /** always-dark variant (viewer) */
  dark?: boolean;
  z?: number;
}) {
  const drag = useDragControls();
  useBackHandler(open, onClose, BACK.sheet, 'sheet');
  const panel = useDialogFocus<HTMLDivElement>(open);
  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div className="fixed inset-0 flex items-end justify-center" style={{ zIndex: z }} initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onKeyDown={(e) => e.key === 'Escape' && onClose()}>
          <div className="absolute inset-0 bg-black/45" onClick={onClose} />
          <motion.div
            ref={panel}
            tabIndex={-1}
            role="dialog"
            aria-modal="true"
            aria-label={typeof title === 'string' ? title : undefined}
            initial={{ y: '100%' }}
            animate={{ y: 0 }}
            exit={{ y: '100%' }}
            transition={{ type: 'spring', stiffness: 440, damping: 42 }}
            drag="y"
            dragListener={false}
            dragControls={drag}
            dragConstraints={{ top: 0, bottom: 0 }}
            dragElastic={{ top: 0, bottom: 0.8 }}
            onDragEnd={(_, i) => {
              if (i.offset.y > 90 || i.velocity.y > 550) onClose();
            }}
            className={cn(
              'relative flex max-h-[85dvh] w-full max-w-xl flex-col overflow-hidden rounded-t-[28px] border border-b-0 shadow-soft outline-none',
              dark ? 'border-white/10 bg-[#16161d] text-white' : 'border-line bg-elev text-fg',
              className,
            )}
          >
            <div className="flex h-7 shrink-0 cursor-grab touch-none items-center justify-center" onPointerDown={(e) => drag.start(e)}>
              <span className={cn('h-1.5 w-10 rounded-full', dark ? 'bg-white/25' : 'bg-line-strong')} />
            </div>
            {title && (
              <div className="shrink-0 touch-none px-5 pb-2 font-display text-lg font-semibold" onPointerDown={(e) => drag.start(e)}>
                {title}
              </div>
            )}
            <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain pb-[max(12px,env(safe-area-inset-bottom))]">{children}</div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}

/** Big touch-friendly row for sheets. */
export function SheetItem({ icon, children, onClick, danger, hint }: { icon?: ReactNode; children: ReactNode; onClick?: () => void; danger?: boolean; hint?: ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'flex min-h-[52px] w-full items-center gap-4 px-5 text-left text-[15px] transition-colors active:bg-surface-2 [&>svg]:size-5 [&>svg]:shrink-0',
        danger ? 'text-danger' : '',
      )}
    >
      {icon}
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {hint && <span className="text-sm text-faint">{hint}</span>}
    </button>
  );
}
