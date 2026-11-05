import { AnimatePresence, motion } from 'motion/react';
import { CopyPlus, Plus, Trash2, Type } from 'lucide-react';
import { Link } from 'react-router';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { createTextLayer, type TextLayer } from '@/lib/text';
import { Button } from '@/components/ui';
import { TextLayerControls } from '@/components/TextLayerControls';
import { clamp, outDuration, srcToOut, textWindow } from './model';
import { PanelSection, useDesignSize } from './panels';
import { useEditor, usePlayhead } from './store';
import { fmtTime } from './Timeline';

/** Text layers: list, timing on the output timeline and full typography controls (custom fonts included). */
export function TextPanel() {
  const t = useT();
  const st = useEditor((s) => s.s);
  const selected = useEditor((s) => s.selectedText);
  const set = useEditor((s) => s.set);
  const select = useEditor((s) => s.select);
  const design = useDesignSize();
  const dur = outDuration(st);
  const layer = st.texts.find((l) => l.id === selected) ?? null;
  const now = () => clamp(srcToOut(st, usePlayhead.getState().time), 0, dur);

  const patch = (id: string, p: Partial<TextLayer>) => set((s) => ({ texts: s.texts.map((l) => (l.id === id ? { ...l, ...p } : l)) }));
  const add = () => {
    const l = createTextLayer({
      text: t('video.textDefault'),
      x: design.w / 2,
      y: design.h * 0.78,
      fontSize: Math.max(12, Math.round(Math.min(design.w, design.h) * 0.09)),
      start: 0,
      end: dur,
    });
    set((s) => ({ texts: [...s.texts, l] }));
    select(l.id);
  };
  const duplicate = (l: TextLayer) => {
    const { id: _old, ...rest } = l;
    const c = createTextLayer({ ...rest, x: Math.min(design.w, l.x + design.w * 0.04), y: Math.min(design.h, l.y + design.h * 0.04) });
    set((s) => ({ texts: [...s.texts, c] }));
    select(c.id);
  };
  const remove = (id: string) => {
    set((s) => ({ texts: s.texts.filter((l) => l.id !== id) }));
    if (selected === id) select(null);
  };

  return (
    <>
      <PanelSection
        title={t('video.texts')}
        hint={t('video.textsHint')}
        action={
          <Button size="sm" variant="primary" icon={<Plus className="size-4" />} onClick={add} data-testid="add-text">
            {t('common.add')}
          </Button>
        }
      >
        {st.texts.length === 0 && (
          <button
            type="button"
            onClick={add}
            className="flex flex-col items-center gap-2 rounded-2xl border border-dashed border-line-strong px-4 py-6 text-center text-sm text-dim transition-colors hover:border-[var(--accent)] hover:text-fg"
          >
            <Type className="size-6 text-accent" />
            {t('video.addFirstText')}
          </button>
        )}
        <div className="flex flex-col gap-1.5">
          <AnimatePresence initial={false}>
            {st.texts.map((l) => {
              const w = textWindow(l, dur);
              const active = l.id === selected;
              return (
                <motion.div
                  key={l.id}
                  layout
                  initial={{ opacity: 0, height: 0 }}
                  animate={{ opacity: 1, height: 'auto' }}
                  exit={{ opacity: 0, height: 0 }}
                  className={cn('group flex cursor-pointer items-center gap-2 rounded-xl border px-2.5 py-2 transition-colors', active ? 'border-[color-mix(in_oklab,var(--accent)_55%,transparent)] bg-[color-mix(in_oklab,var(--accent)_12%,transparent)]' : 'border-line hover:bg-surface-2')}
                  onClick={() => select(l.id)}
                >
                  <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-surface-2 text-[15px]" style={{ fontFamily: `"${l.fontFamily}"`, color: l.color }}>
                    Aa
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[13px] font-medium" style={{ fontFamily: `"${l.fontFamily}"` }}>
                      {l.text.split('\n')[0] || '…'}
                    </div>
                    <div className="text-[11px] text-faint tabular-nums">
                      {fmtTime(w.start)} – {fmtTime(w.end)}
                    </div>
                  </div>
                  <button type="button" title={t('video.duplicate')} className="grid size-7 place-items-center rounded-lg text-faint opacity-0 group-hover:opacity-100 hover:bg-surface-3 hover:text-fg" onClick={(e) => (e.stopPropagation(), duplicate(l))}>
                    <CopyPlus className="size-3.5" />
                  </button>
                  <button type="button" title={t('common.delete')} className="grid size-7 place-items-center rounded-lg text-faint hover:bg-danger/10 hover:text-danger" onClick={(e) => (e.stopPropagation(), remove(l.id))}>
                    <Trash2 className="size-3.5" />
                  </button>
                </motion.div>
              );
            })}
          </AnimatePresence>
        </div>
      </PanelSection>

      <AnimatePresence mode="wait">
        {layer && (
          <motion.div key={layer.id} initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8 }} transition={{ duration: 0.18 }}>
            <PanelSection title={t('video.textTiming')} hint={t('video.textTimingHint')}>
              <div className="flex gap-2">
                <TimeInput label={t('video.from')} value={textWindow(layer, dur).start} max={dur} onChange={(start) => patch(layer.id, { start, end: Math.max(start + 0.1, textWindow(layer, dur).end) })} />
                <TimeInput label={t('video.to')} value={textWindow(layer, dur).end} max={dur} onChange={(end) => patch(layer.id, { end, start: Math.min(textWindow(layer, dur).start, end - 0.1) })} />
              </div>
              <div className="flex flex-wrap gap-1.5">
                <Button size="sm" variant="secondary" onClick={() => patch(layer.id, { start: Math.min(now(), textWindow(layer, dur).end - 0.1) })}>
                  {t('video.startHere')}
                </Button>
                <Button size="sm" variant="secondary" onClick={() => patch(layer.id, { end: Math.max(now(), textWindow(layer, dur).start + 0.1) })}>
                  {t('video.endHere')}
                </Button>
                <Button size="sm" variant="ghost" onClick={() => patch(layer.id, { start: 0, end: dur })}>
                  {t('video.wholeClip')}
                </Button>
              </div>
            </PanelSection>
            <PanelSection
              title={t('video.typography')}
              hint={
                <>
                  {t('video.fontsHint')}{' '}
                  <Link to="/fonts" className="text-accent hover:underline">
                    {t('nav.fonts')}
                  </Link>
                </>
              }
            >
              <TextLayerControls layer={layer} onChange={(p) => patch(layer.id, p)} maxSize={Math.max(60, Math.round(design.h * 0.4))} />
            </PanelSection>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  );
}

function TimeInput({ label, value, max, onChange }: { label: string; value: number; max: number; onChange: (v: number) => void }) {
  return (
    <label className="flex flex-1 flex-col gap-1">
      <span className="text-xs text-faint">{label}</span>
      <input
        type="number"
        min={0}
        max={max}
        step={0.1}
        value={Number(value.toFixed(2))}
        onChange={(e) => {
          const v = Number(e.target.value);
          if (Number.isFinite(v)) onChange(clamp(v, 0, max));
        }}
        className="h-9 w-full rounded-xl border border-line bg-surface px-3 font-mono text-sm tabular-nums outline-none focus:border-[color-mix(in_oklab,var(--accent)_60%,transparent)]"
      />
    </label>
  );
}
