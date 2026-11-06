import {
  forwardRef,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
  type RefObject,
} from 'react';
import { createPortal } from 'react-dom';
import { AnimatePresence, motion, useDragControls } from 'motion/react';
import { Check, X, Loader2 } from 'lucide-react';
import { cn } from '@/lib/cn';
import { BACK, useBackHandler } from '@/lib/backStack';
import { useIsMobile } from '@/lib/useMedia';
import { useDialogFocus } from '@/lib/useDialogFocus';
import { useT } from '@/i18n';

// ───────────────────────── Button ─────────────────────────

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'glass';
type Size = 'sm' | 'md' | 'lg';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  icon?: ReactNode;
  loading?: boolean;
}

const variants: Record<Variant, string> = {
  primary: 'bg-accent-gradient text-white shadow-[0_6px_20px_-6px_var(--accent)] hover:brightness-110',
  secondary: 'bg-surface-2 text-fg hover:bg-surface-3 border border-line',
  ghost: 'text-dim hover:text-fg hover:bg-surface-2',
  danger: 'bg-danger/90 text-white hover:bg-danger',
  glass: 'glass text-fg border border-line hover:bg-surface-3',
};
const sizes: Record<Size, string> = {
  sm: 'h-8 px-3 text-[13px] gap-1.5 rounded-lg',
  md: 'h-10 px-4 text-sm gap-2 rounded-xl',
  lg: 'h-12 px-6 text-[15px] gap-2.5 rounded-2xl',
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'secondary', size = 'md', icon, loading, className, children, disabled, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      disabled={disabled || loading}
      className={cn(
        'inline-flex select-none items-center justify-center font-medium whitespace-nowrap transition-[transform,background,filter,color,box-shadow] duration-150 active:scale-[0.97] disabled:pointer-events-none disabled:opacity-50',
        variants[variant],
        sizes[size],
        className,
      )}
      {...rest}
    >
      {loading ? <Loader2 className="size-4 animate-spin" /> : icon}
      {children}
    </button>
  );
});

export const IconButton = forwardRef<HTMLButtonElement, ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean; size?: 'sm' | 'md' | 'lg'; tone?: 'default' | 'glass' }>(
  function IconButton({ label, active, size = 'md', tone = 'default', className, children, ...rest }, ref) {
    const s = size === 'sm' ? 'size-8 rounded-lg' : size === 'lg' ? 'size-12 rounded-2xl' : 'size-10 rounded-xl';
    return (
      <button
        ref={ref}
        aria-label={label}
        title={label}
        className={cn(
          'touch-target inline-flex shrink-0 items-center justify-center transition-[transform,background,color] duration-150 active:scale-90 disabled:opacity-40 disabled:pointer-events-none [&>svg]:size-[18px]',
          s,
          tone === 'glass' ? 'glass border border-line text-fg hover:bg-surface-3' : active ? 'bg-surface-3 text-fg' : 'text-dim hover:bg-surface-2 hover:text-fg',
          className,
        )}
        {...rest}
      >
        {children}
      </button>
    );
  },
);

// ───────────────────────── Inputs ─────────────────────────

export const Input = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement> & { icon?: ReactNode }>(function Input(
  { className, icon, ...rest },
  ref,
) {
  return (
    <div className={cn('relative flex items-center', className)}>
      {icon && <span className="pointer-events-none absolute left-3 text-faint [&>svg]:size-4">{icon}</span>}
      <input
        ref={ref}
        className={cn(
          'h-10 w-full rounded-xl border border-line bg-surface px-3 text-sm text-fg outline-none transition placeholder:text-faint focus:border-[color-mix(in_oklab,var(--accent)_60%,transparent)] focus:bg-surface-2',
          icon && 'pl-9',
        )}
        {...rest}
      />
    </div>
  );
});

export function Field({ label, hint, children, className }: { label: ReactNode; hint?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <label className={cn('flex flex-col gap-1.5', className)}>
      <span className="text-[13px] font-medium text-dim">{label}</span>
      {children}
      {hint && <span className="text-xs text-faint">{hint}</span>}
    </label>
  );
}

export function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label?: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        'relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 disabled:opacity-50',
        checked ? 'bg-accent' : 'bg-surface-3',
      )}
    >
      <motion.span
        layout
        transition={{ type: 'spring', stiffness: 600, damping: 35 }}
        className={cn('block size-5 rounded-full bg-white shadow', checked ? 'ml-[22px]' : 'ml-0.5')}
      />
    </button>
  );
}

export function Checkbox({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label: ReactNode; hint?: ReactNode }) {
  return (
    <label className="flex cursor-pointer select-none items-start gap-3">
      <button
        type="button"
        role="checkbox"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={cn(
          'mt-0.5 grid size-5 shrink-0 place-items-center rounded-md border transition-colors',
          checked ? 'border-transparent bg-accent text-white' : 'border-line-strong bg-surface',
        )}
      >
        <AnimatePresence>
          {checked && (
            <motion.span initial={{ scale: 0 }} animate={{ scale: 1 }} exit={{ scale: 0 }} transition={{ type: 'spring', stiffness: 700, damping: 30 }}>
              <Check className="size-3.5" strokeWidth={3} />
            </motion.span>
          )}
        </AnimatePresence>
      </button>
      <span className="flex flex-col">
        <span className="text-sm text-fg">{label}</span>
        {hint && <span className="text-xs text-faint">{hint}</span>}
      </span>
    </label>
  );
}

export function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
  onCommit,
  format,
  center,
  className,
}: {
  label?: ReactNode;
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (v: number) => void;
  onCommit?: (v: number) => void;
  format?: (v: number) => string;
  /** value that double click resets to */
  center?: number;
  className?: string;
}) {
  const fill = ((value - min) / (max - min)) * 100;
  const id = useId();
  return (
    <div className={cn('flex flex-col gap-1', className)}>
      {label !== undefined && (
        <div className="flex items-center justify-between text-[13px]">
          <label htmlFor={id} className="text-dim">
            {label}
          </label>
          <span className="tabular-nums text-faint">{format ? format(value) : value}</span>
        </div>
      )}
      <input
        id={id}
        type="range"
        className="peek-range"
        min={min}
        max={max}
        step={step}
        value={value}
        style={{ ['--fill' as string]: `${fill}%` }}
        onChange={(e) => onChange(Number(e.target.value))}
        onPointerUp={(e) => onCommit?.(Number((e.target as HTMLInputElement).value))}
        onKeyUp={(e) => onCommit?.(Number((e.target as HTMLInputElement).value))}
        onDoubleClick={() => {
          if (center !== undefined) {
            onChange(center);
            onCommit?.(center);
          }
        }}
      />
    </div>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  size = 'md',
  className,
}: {
  value: T;
  options: { value: T; label: ReactNode; icon?: ReactNode; title?: string }[];
  onChange: (v: T) => void;
  size?: 'sm' | 'md';
  className?: string;
}) {
  const id = useId();
  return (
    <div data-segmented className={cn('inline-flex rounded-xl border border-line bg-surface p-0.5', className)}>
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          title={o.title}
          onClick={() => onChange(o.value)}
          className={cn(
            'relative inline-flex items-center justify-center gap-1.5 font-medium transition-colors [&>svg]:size-4',
            size === 'sm' ? 'h-7 px-2.5 text-xs rounded-[9px]' : 'h-8 px-3 text-[13px] rounded-[10px]',
            value === o.value ? 'text-fg' : 'text-faint hover:text-dim',
          )}
        >
          {value === o.value && (
            <motion.span
              layoutId={`seg-${id}`}
              className="absolute inset-0 rounded-[inherit] bg-surface-3 shadow-sm"
              transition={{ type: 'spring', stiffness: 500, damping: 38 }}
            />
          )}
          <span className="relative z-10 inline-flex items-center gap-1.5">
            {o.icon}
            {o.label}
          </span>
        </button>
      ))}
    </div>
  );
}

export function ColorSwatches({ value, onChange, colors }: { value: string; onChange: (c: string) => void; colors: string[] }) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      {colors.map((c) => (
        <button
          key={c}
          type="button"
          aria-label={c}
          onClick={() => onChange(c)}
          className={cn('size-7 rounded-full transition-transform hover:scale-110', value.toLowerCase() === c.toLowerCase() && 'ring-accent')}
          style={{ background: c }}
        />
      ))}
      <label className="relative grid size-7 cursor-pointer place-items-center overflow-hidden rounded-full border border-dashed border-line-strong text-faint hover:text-fg" title="Custom">
        <span className="text-lg leading-none">+</span>
        <input type="color" value={value} onChange={(e) => onChange(e.target.value)} className="absolute inset-0 cursor-pointer opacity-0" />
      </label>
    </div>
  );
}

export function ColorInput({ value, onChange, label }: { value: string; onChange: (c: string) => void; label?: string }) {
  return (
    <label className="inline-flex cursor-pointer items-center gap-2 rounded-xl border border-line bg-surface px-2 py-1.5 text-[13px] text-dim hover:bg-surface-2">
      <span className="relative size-5 overflow-hidden rounded-md border border-line" style={{ background: value }}>
        <input type="color" value={toHex(value)} onChange={(e) => onChange(e.target.value)} className="absolute inset-0 cursor-pointer opacity-0" />
      </span>
      {label && <span>{label}</span>}
    </label>
  );
}

function toHex(c: string): string {
  if (/^#[0-9a-f]{6}$/i.test(c)) return c;
  const m = c.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/i);
  if (m) return `#${[m[1], m[2], m[3]].map((x) => Number(x).toString(16).padStart(2, '0')).join('')}`;
  return '#ffffff';
}

// ───────────────────────── Overlays ─────────────────────────

export function Modal({
  open,
  onClose,
  title,
  children,
  footer,
  width = 480,
  className,
}: {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
  className?: string;
}) {
  const sheet = useIsMobile();
  const drag = useDragControls();
  const t = useT();
  const panel = useDialogFocus<HTMLDivElement>(open);
  useBackHandler(open, onClose, BACK.modal, 'modal');
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [open, onClose]);
  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div
          className={cn('fixed inset-0 z-[80] flex justify-center', sheet ? 'items-end' : 'items-center p-4')}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
        >
          <motion.div className="absolute inset-0 bg-[var(--overlay)] backdrop-blur-sm" onClick={onClose} />
          <motion.div
            ref={panel}
            tabIndex={-1}
            role="dialog"
            aria-modal="true"
            aria-label={typeof title === 'string' ? title : undefined}
            initial={sheet ? { y: '100%' } : { opacity: 0, y: 24, scale: 0.96 }}
            animate={sheet ? { y: 0 } : { opacity: 1, y: 0, scale: 1 }}
            exit={sheet ? { y: '100%' } : { opacity: 0, y: 12, scale: 0.97 }}
            transition={sheet ? { type: 'spring', stiffness: 420, damping: 40 } : { type: 'spring', stiffness: 420, damping: 32 }}
            drag={sheet ? 'y' : false}
            dragListener={false}
            dragControls={drag}
            dragConstraints={{ top: 0, bottom: 0 }}
            dragElastic={{ top: 0, bottom: 0.8 }}
            onDragEnd={(_, i) => {
              if (i.offset.y > 110 || i.velocity.y > 600) onClose();
            }}
            style={{ maxWidth: sheet ? undefined : width }}
            className={cn(
              'relative flex w-full flex-col overflow-hidden border border-line bg-elev shadow-soft outline-none',
              sheet ? 'max-h-[92dvh] rounded-t-[28px] border-b-0 pb-[env(safe-area-inset-bottom)]' : 'max-h-[88vh] rounded-3xl',
              className,
            )}
          >
            {sheet && (
              <div className="flex h-6 shrink-0 cursor-grab touch-none items-center justify-center" onPointerDown={(e) => drag.start(e)}>
                <span className="h-1.5 w-10 rounded-full bg-line-strong" />
              </div>
            )}
            {title && (
              <div className={cn('flex items-center justify-between gap-4 px-6 pb-2', sheet ? 'touch-none pt-1' : 'pt-5')} onPointerDown={sheet ? (e) => drag.start(e) : undefined}>
                <h2 className="font-display text-lg font-semibold">{title}</h2>
                <IconButton label={t('common.close')} size="sm" onClick={onClose} onPointerDown={(e) => e.stopPropagation()}>
                  <X />
                </IconButton>
              </div>
            )}
            <div className="min-h-0 flex-1 overflow-auto overscroll-contain px-6 py-3">{children}</div>
            {footer && <div className="flex flex-wrap items-center justify-end gap-2 border-t border-line px-6 py-4">{footer}</div>}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}

/** Anchored popover that closes on outside click / Escape. */
export function Popover({
  open,
  onClose,
  anchor,
  children,
  align = 'start',
  side = 'bottom',
  className,
}: {
  open: boolean;
  onClose: () => void;
  anchor: RefObject<HTMLElement | null>;
  children: ReactNode;
  align?: 'start' | 'end' | 'center';
  side?: 'bottom' | 'top';
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ x: number; y: number; origin: string; maxH?: number } | null>(null);
  useBackHandler(open, onClose, BACK.sheet, 'popover');
  useLayoutEffect(() => {
    if (!open || !anchor.current) return;
    const place = () => {
      const r = anchor.current!.getBoundingClientRect();
      const el = ref.current;
      const w = el?.offsetWidth ?? 240;
      // natural height (without a previous max-height)
      const h = el?.scrollHeight ?? 200;
      const vh = window.innerHeight;
      let x = align === 'end' ? r.right - w : align === 'center' ? r.left + r.width / 2 - w / 2 : r.left;
      x = Math.max(8, Math.min(window.innerWidth - w - 8, x));
      const below = vh - 8 - (r.bottom + 8);
      const above = r.top - 8 - 8;
      // the roomier side when it fits nowhere: never over the anchor (it would hide the button that opened it)
      const down = side === 'bottom' ? h <= below || below >= above : !(h <= above || above >= below);
      const maxH = Math.max(120, down ? below : above);
      const y = down ? r.bottom + 8 : Math.max(8, r.top - 8 - Math.min(h, maxH));
      setPos({ x, y, maxH: h > maxH ? maxH : undefined, origin: `${align === 'end' ? 'right' : align === 'center' ? 'center' : 'left'} ${down ? 'top' : 'bottom'}` });
    };
    place();
    const raf = requestAnimationFrame(place);
    window.addEventListener('resize', place);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener('resize', place);
    };
  }, [open, anchor, align, side]);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (ref.current?.contains(t) || anchor.current?.contains(t)) return;
      onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('pointerdown', onDown, true);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('pointerdown', onDown, true);
      window.removeEventListener('keydown', onKey);
    };
  }, [open, onClose, anchor]);
  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div
          ref={ref}
          initial={{ opacity: 0, scale: 0.94, y: side === 'top' ? 6 : -6 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.96 }}
          transition={{ type: 'spring', stiffness: 520, damping: 34 }}
          style={{ left: pos?.x ?? -9999, top: pos?.y ?? -9999, transformOrigin: pos?.origin, ...(pos?.maxH ? { maxHeight: pos.maxH, overflowY: 'auto', overscrollBehavior: 'contain' } : null) }}
          className={cn('fixed z-[90] min-w-48 rounded-2xl border border-line bg-elev p-1.5 shadow-soft', className)}
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}

export function MenuItem({
  icon,
  children,
  onClick,
  danger,
  checked,
  disabled,
  hint,
}: {
  icon?: ReactNode;
  children: ReactNode;
  onClick?: () => void;
  danger?: boolean;
  checked?: boolean;
  disabled?: boolean;
  hint?: ReactNode;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      className={cn(
        'flex w-full items-center gap-2.5 rounded-xl px-2.5 py-2 text-left text-[13px] transition-colors disabled:opacity-40 [&>svg]:size-4 [&>svg]:shrink-0',
        danger ? 'text-danger hover:bg-danger/10' : 'text-fg hover:bg-surface-2',
      )}
    >
      {icon}
      <span className="flex-1">{children}</span>
      {hint && <span className="text-xs text-faint">{hint}</span>}
      {checked && <Check className="size-4 text-accent" />}
    </button>
  );
}

export function MenuLabel({ children }: { children: ReactNode }) {
  return <div className="px-2.5 pt-2 pb-1 text-[11px] font-semibold tracking-wider text-faint uppercase">{children}</div>;
}

export function MenuSep() {
  return <div className="my-1 h-px bg-line" />;
}

/** Fixed-position context menu at the pointer. */
export function ContextMenu({ at, onClose, children }: { at: { x: number; y: number } | null; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState(at);
  useBackHandler(!!at, onClose, BACK.sheet, 'menu');
  useLayoutEffect(() => {
    if (!at) return;
    const el = ref.current;
    const w = el?.offsetWidth ?? 220;
    const h = el?.offsetHeight ?? 300;
    setPos({ x: Math.min(at.x, window.innerWidth - w - 8), y: Math.min(at.y, window.innerHeight - h - 8) });
  }, [at]);
  useEffect(() => {
    if (!at) return;
    const onDown = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('pointerdown', onDown, true);
    window.addEventListener('keydown', onKey);
    window.addEventListener('blur', onClose);
    return () => {
      window.removeEventListener('pointerdown', onDown, true);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('blur', onClose);
    };
  }, [at, onClose]);
  return createPortal(
    <AnimatePresence>
      {at && (
        <motion.div
          ref={ref}
          initial={{ opacity: 0, scale: 0.92 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.95, transition: { duration: 0.1 } }}
          transition={{ type: 'spring', stiffness: 600, damping: 36 }}
          style={{ left: pos?.x ?? at.x, top: pos?.y ?? at.y, transformOrigin: 'top left' }}
          className="fixed z-[95] min-w-52 rounded-2xl border border-line bg-elev p-1.5 shadow-soft"
          onClick={onClose}
          onContextMenu={(e) => e.preventDefault()}
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}

// ───────────────────────── Feedback ─────────────────────────

export function Spinner({ className }: { className?: string }) {
  return <Loader2 className={cn('size-5 animate-spin text-dim', className)} />;
}

export function ProgressRing({ value, size = 28, stroke = 3 }: { value: number; size?: number; stroke?: number }) {
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  return (
    <svg width={size} height={size} className="-rotate-90">
      <circle cx={size / 2} cy={size / 2} r={r} stroke="var(--surface-3)" strokeWidth={stroke} fill="none" />
      <motion.circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        stroke="var(--accent)"
        strokeWidth={stroke}
        fill="none"
        strokeLinecap="round"
        strokeDasharray={c}
        animate={{ strokeDashoffset: c * (1 - Math.max(0, Math.min(1, value))) }}
        transition={{ type: 'spring', stiffness: 120, damping: 20 }}
      />
    </svg>
  );
}

export function ProgressBar({ value, className }: { value: number; className?: string }) {
  return (
    <div className={cn('h-1.5 w-full overflow-hidden rounded-full bg-surface-3', className)}>
      <motion.div
        className="h-full rounded-full bg-accent-gradient"
        animate={{ width: `${Math.max(0, Math.min(1, value)) * 100}%` }}
        transition={{ type: 'spring', stiffness: 120, damping: 22 }}
      />
    </div>
  );
}

export function EmptyState({ icon, title, body, action }: { icon: ReactNode; title: ReactNode; body?: ReactNode; action?: ReactNode }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: 'spring', stiffness: 260, damping: 26 }}
      className="mx-auto flex max-w-sm flex-col items-center px-6 py-16 text-center"
    >
      <div className="relative mb-5 grid size-20 place-items-center rounded-3xl border border-line bg-surface text-dim [&>svg]:size-9">
        <div className="absolute inset-0 -z-10 rounded-3xl bg-accent-gradient opacity-20 blur-2xl" />
        {icon}
      </div>
      <h3 className="font-display text-xl font-semibold">{title}</h3>
      {body && <p className="mt-2 text-sm text-dim">{body}</p>}
      {action && <div className="mt-6">{action}</div>}
    </motion.div>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="rounded-md border border-line bg-surface px-1.5 py-0.5 font-sans text-[11px] text-dim">{children}</kbd>;
}

export function Badge({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn('inline-flex items-center gap-1 rounded-full bg-surface-2 px-2 py-0.5 text-[11px] font-medium text-dim', className)}>{children}</span>;
}

export function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel,
  cancelLabel,
  danger,
  onConfirm,
  onClose,
}: {
  open: boolean;
  title: ReactNode;
  body?: ReactNode;
  confirmLabel: string;
  cancelLabel: string;
  danger?: boolean;
  onConfirm: () => void | Promise<void>;
  onClose: () => void;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      width={420}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {cancelLabel}
          </Button>
          <Button
            variant={danger ? 'danger' : 'primary'}
            loading={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await onConfirm();
                onClose();
              } finally {
                setBusy(false);
              }
            }}
          >
            {confirmLabel}
          </Button>
        </>
      }
    >
      {body && <p className="pb-2 text-sm text-dim">{body}</p>}
    </Modal>
  );
}

export { BottomSheet, SheetItem } from './sheet';
