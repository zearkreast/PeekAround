import { motion } from 'motion/react';
import { Brush, Eraser, Undo2 } from 'lucide-react';
import { useT } from '@/i18n';
import { Button, ColorSwatches, Slider } from '@/components/ui';
import { useEditor } from '../store';
import { PanelHeading, sectionMotion } from '../ToolTabs';

const COLORS = ['#ffffff', '#111111', '#f43f5e', '#f97316', '#facc15', '#22c55e', '#06b6d4', '#3b82f6', '#8b5cf6', '#ec4899'];

export function DrawPanel() {
  const t = useT();
  const brush = useEditor((s) => s.brush);
  const setBrush = useEditor((s) => s.setBrush);
  const strokes = useEditor((s) => s.h.doc.strokes);
  const change = useEditor((s) => s.change);

  return (
    <div className="flex flex-col gap-6">
      <motion.div {...sectionMotion(0)} className="flex items-center gap-3 rounded-2xl border border-line bg-surface p-3">
        <span className="relative grid size-14 shrink-0 place-items-center overflow-hidden rounded-xl bg-[repeating-conic-gradient(#2a2a33_0%_25%,#1c1c23_0%_50%)] bg-[length:12px_12px]">
          <motion.span
            className="rounded-full"
            animate={{ width: Math.max(4, Math.min(44, brush.size * 0.9)), height: Math.max(4, Math.min(44, brush.size * 0.9)), opacity: brush.opacity }}
            transition={{ type: 'spring', stiffness: 500, damping: 30 }}
            style={{ background: brush.color }}
          />
        </span>
        <div className="flex items-start gap-2 text-sm text-dim">
          <Brush className="mt-0.5 size-4 shrink-0 text-accent" />
          {t('photo.draw.hint')}
        </div>
      </motion.div>

      <motion.section {...sectionMotion(1)}>
        <PanelHeading>{t('photo.draw.color')}</PanelHeading>
        <ColorSwatches value={brush.color} onChange={(color) => setBrush({ color })} colors={COLORS} />
      </motion.section>

      <motion.section {...sectionMotion(2)} className="flex flex-col gap-3">
        <Slider label={t('photo.draw.size')} value={brush.size} min={2} max={100} onChange={(size) => setBrush({ size })} center={12} />
        <Slider label={t('photo.draw.opacity')} value={Math.round(brush.opacity * 100)} min={10} max={100} format={(v) => `${v}%`} onChange={(v) => setBrush({ opacity: v / 100 })} center={100} />
      </motion.section>

      <motion.section {...sectionMotion(3)} className="flex flex-wrap items-center justify-between gap-2 border-t border-line pt-4">
        <span className="text-xs text-faint tabular-nums">{t('photo.draw.strokes', { n: strokes.length })}</span>
        <div className="flex gap-1.5">
          <Button size="sm" variant="ghost" icon={<Undo2 className="size-3.5" />} disabled={!strokes.length} onClick={() => change((d) => ({ ...d, strokes: d.strokes.slice(0, -1) }))}>
            {t('photo.draw.undo')}
          </Button>
          <Button size="sm" variant="ghost" icon={<Eraser className="size-3.5" />} disabled={!strokes.length} onClick={() => change((d) => ({ ...d, strokes: [] }))}>
            {t('photo.draw.clear')}
          </Button>
        </div>
      </motion.section>
    </div>
  );
}
