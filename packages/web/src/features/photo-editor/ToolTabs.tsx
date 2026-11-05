import { useId, type ReactNode } from 'react';
import { motion } from 'motion/react';
import { cn } from '@/lib/cn';

export interface ToolTab<T extends string> {
  id: T;
  label: string;
  icon: ReactNode;
}

/** Icon + label tabs with a springy shared-layout indicator (photo editor & collage). */
export function ToolTabs<T extends string>({ tabs, value, onChange, className }: { tabs: ToolTab<T>[]; value: T; onChange: (t: T) => void; className?: string }) {
  const id = useId();
  return (
    <div role="tablist" className={cn('flex gap-1 rounded-2xl border border-line bg-surface p-1', className)}>
      {tabs.map((tb) => {
        const on = tb.id === value;
        return (
          <button
            key={tb.id}
            type="button"
            role="tab"
            aria-selected={on}
            onClick={() => onChange(tb.id)}
            className={cn(
              'relative flex min-w-0 flex-1 flex-col items-center gap-1 rounded-xl px-1 pt-2 pb-1.5 text-[11px] font-medium transition-colors [&_svg]:size-[18px]',
              on ? 'text-fg' : 'text-faint hover:text-dim',
            )}
          >
            {on && (
              <motion.span
                layoutId={`tab-bg-${id}`}
                className="absolute inset-0 rounded-xl bg-surface-3 shadow-sm"
                transition={{ type: 'spring', stiffness: 520, damping: 38 }}
              />
            )}
            <motion.span className="relative z-10" animate={{ scale: on ? 1.08 : 1, y: on ? -1 : 0 }} transition={{ type: 'spring', stiffness: 600, damping: 26 }}>
              {tb.icon}
            </motion.span>
            <span className="relative z-10 max-w-full truncate">{tb.label}</span>
            {on && (
              <motion.span
                layoutId={`tab-line-${id}`}
                className="absolute -bottom-px left-1/2 z-10 h-[3px] w-6 -translate-x-1/2 rounded-full bg-accent-gradient"
                transition={{ type: 'spring', stiffness: 520, damping: 38 }}
              />
            )}
          </button>
        );
      })}
    </div>
  );
}

/** Small uppercase section heading used in tool panels. */
export function PanelHeading({ children, action }: { children: ReactNode; action?: ReactNode }) {
  return (
    <div className="mb-2.5 flex items-center justify-between gap-2">
      <h3 className="text-[11px] font-semibold tracking-wider text-faint uppercase">{children}</h3>
      {action}
    </div>
  );
}

/** Staggered entrance for panel sections. */
export const sectionMotion = (i: number) => ({
  initial: { opacity: 0, y: 12 },
  animate: { opacity: 1, y: 0 },
  transition: { type: 'spring' as const, stiffness: 380, damping: 32, delay: i * 0.045 },
});
