import { useEffect, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { ArrowLeft, Download, Eye, Maximize, Redo2, ScanSearch, ShieldCheck, Undo2 } from 'lucide-react';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { useSettings } from '@/lib/queries';
import { Button, IconButton, Popover, ProgressBar, Segmented, Slider } from '@/components/ui';
import { useFrame, useEnv } from './context';
import { useEditor } from './store';

export type SaveFormat = 'jpg' | 'png' | 'webp';

const PREF_KEY = 'peek.photo.save';
function loadPrefs(): { format: SaveFormat; quality: number } {
  try {
    const v = JSON.parse(localStorage.getItem(PREF_KEY) ?? 'null');
    if (v && ['jpg', 'png', 'webp'].includes(v.format)) return { format: v.format, quality: Math.min(100, Math.max(40, Number(v.quality) || 92)) };
  } catch {
    /* storage unavailable */
  }
  return { format: 'jpg', quality: 92 };
}
function savePrefs(p: { format: SaveFormat; quality: number }) {
  try {
    localStorage.setItem(PREF_KEY, JSON.stringify(p));
  } catch {
    /* ignore */
  }
}

export interface SaveState {
  progress: number; // 0..1
  label: string;
}

/** Format + quality popover with a live file name preview and the "copy, original untouched" note. */
function SaveControl({ onSave, saving, openSignal }: { onSave: (format: SaveFormat, quality: number) => void; saving: SaveState | null; openSignal: number }) {
  const t = useT();
  const env = useEnv();
  const { crop } = useFrame();
  const settings = useSettings();
  const anchor = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [prefs, setPrefs] = useState(loadPrefs);
  useEffect(() => {
    if (openSignal) setOpen(true);
  }, [openSignal]);
  const wasSaving = useRef(false);
  useEffect(() => {
    if (saving) setOpen(true);
    else if (wasSaving.current) setOpen(false); // done → close
    wasSaving.current = !!saving;
  }, [saving]);

  const base = env.item.name.replace(/\.[^.]+$/, '');
  const suffix = settings.data?.editSuffix ?? '_edit';
  const update = (p: Partial<typeof prefs>) => {
    const next = { ...prefs, ...p };
    setPrefs(next);
    savePrefs(next);
  };

  return (
    <>
      <Button ref={anchor} variant="primary" icon={<Download className="size-4" />} onClick={() => setOpen((o) => !o)} disabled={!env.source}>
        <span className="hidden sm:inline">{t('photo.save')}</span>
        <span className="sm:hidden">{t('common.save')}</span>
      </Button>
      <Popover open={open} onClose={() => !saving && setOpen(false)} anchor={anchor} align="end" className="w-80 p-4">
        <div className="flex flex-col gap-4">
          <div>
            <div className="mb-1.5 text-[13px] font-medium text-dim">{t('photo.format')}</div>
            <Segmented
              value={prefs.format}
              onChange={(format) => update({ format })}
              className="w-full [&>button]:flex-1"
              options={[
                { value: 'jpg', label: 'JPG' },
                { value: 'png', label: 'PNG' },
                { value: 'webp', label: 'WebP' },
              ]}
            />
          </div>
          <motion.div initial={false} animate={{ opacity: prefs.format === 'png' ? 0.35 : 1 }} className={cn(prefs.format === 'png' && 'pointer-events-none')}>
            <Slider label={t('photo.quality')} value={prefs.quality} min={40} max={100} center={92} format={(v) => `${v}`} onChange={(quality) => update({ quality })} />
          </motion.div>
          <div className="rounded-2xl border border-line bg-surface px-3 py-2.5">
            <div className="text-[11px] font-semibold tracking-wider text-faint uppercase">{t('photo.fileName')}</div>
            <div className="mt-0.5 truncate text-sm font-medium">
              {base}
              <span className="text-gradient">{suffix}</span>.{prefs.format}
            </div>
            <div className="mt-0.5 text-xs text-faint tabular-nums">
              {crop.w} × {crop.h} px
            </div>
          </div>
          <div className="flex items-start gap-2 text-xs text-dim">
            <ShieldCheck className="mt-px size-4 shrink-0 text-ok" />
            {t('tools.copyNote')}
          </div>
          <AnimatePresence mode="wait" initial={false}>
            {saving ? (
              <motion.div key="p" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="flex flex-col gap-2 py-1">
                <div className="flex items-center justify-between text-xs text-dim">
                  <span>{saving.label}</span>
                  <span className="tabular-nums">{Math.round(saving.progress * 100)}%</span>
                </div>
                <ProgressBar value={saving.progress} />
              </motion.div>
            ) : (
              <motion.div key="b" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }}>
                <Button variant="primary" className="w-full" icon={<Download className="size-4" />} onClick={() => onSave(prefs.format, prefs.quality)}>
                  {t('photo.save')}
                </Button>
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </Popover>
    </>
  );
}

export function EditorTopBar({
  onBack,
  onSave,
  saving,
  saveSignal,
}: {
  onBack: () => void;
  onSave: (format: SaveFormat, quality: number) => void;
  saving: SaveState | null;
  saveSignal: number;
}) {
  const t = useT();
  const env = useEnv();
  const canUndo = useEditor((s) => s.h.past.length > 0);
  const canRedo = useEditor((s) => s.h.future.length > 0);
  const undo = useEditor((s) => s.undo);
  const redo = useEditor((s) => s.redo);
  const setCompare = useEditor((s) => s.setCompare);
  const compare = useEditor((s) => s.compare);
  const zoom = useEditor((s) => s.zoom);
  const setZoom = useEditor((s) => s.setZoom);
  const tab = useEditor((s) => s.tab);
  const dirty = useEditor((s) => s.h.doc !== s.saved);

  return (
    <header className="app-drag glass wco-r relative z-20 flex h-14 shrink-0 items-center gap-2 border-b border-line pl-2 [--wco-base:0.5rem] sm:gap-3 sm:pl-3 sm:[--wco-base:0.75rem]">
      <IconButton label={t('common.back')} onClick={onBack}>
        <ArrowLeft />
      </IconButton>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <h1 className="truncate font-display text-[15px] font-semibold">{env.item.name}</h1>
          <AnimatePresence>
            {dirty && (
              <motion.span
                initial={{ scale: 0, opacity: 0 }}
                animate={{ scale: 1, opacity: 1 }}
                exit={{ scale: 0, opacity: 0 }}
                className="size-2 shrink-0 rounded-full bg-accent shadow-[0_0_10px_var(--accent)]"
              />
            )}
          </AnimatePresence>
        </div>
        <div className="hidden items-center gap-1 truncate text-[11px] text-faint md:flex">
          <ShieldCheck className="size-3 shrink-0 text-ok" />
          {t('tools.copyNote')}
        </div>
      </div>

      <div className="flex items-center gap-0.5 rounded-xl border border-line bg-surface p-0.5">
        <IconButton size="sm" label={`${t('common.undo')} (Ctrl+Z)`} disabled={!canUndo} onClick={undo}>
          <Undo2 />
        </IconButton>
        <IconButton size="sm" label={`${t('common.redo')} (Ctrl+Shift+Z)`} disabled={!canRedo} onClick={redo}>
          <Redo2 />
        </IconButton>
        <div className="mx-0.5 h-5 w-px bg-line" />
        <IconButton
          size="sm"
          label={t('photo.compare')}
          active={compare}
          disabled={!env.source}
          onPointerDown={(e) => {
            e.currentTarget.setPointerCapture(e.pointerId);
            setCompare(true);
          }}
          onPointerUp={() => setCompare(false)}
          onPointerCancel={() => setCompare(false)}
          onContextMenu={(e) => e.preventDefault()}
        >
          <Eye />
        </IconButton>
        <div className="mx-0.5 hidden h-5 w-px bg-line sm:block" />
        <IconButton size="sm" className="hidden sm:inline-flex" label={t('photo.zoomFit')} active={zoom === 'fit'} disabled={tab === 'crop'} onClick={() => setZoom('fit')}>
          <Maximize />
        </IconButton>
        <IconButton size="sm" className="hidden sm:inline-flex" label={t('photo.zoom100')} active={zoom === 1} disabled={tab === 'crop'} onClick={() => setZoom(1)}>
          <ScanSearch />
        </IconButton>
      </div>

      <SaveControl onSave={onSave} saving={saving} openSignal={saveSignal} />
    </header>
  );
}
