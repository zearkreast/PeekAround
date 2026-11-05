import { motion } from 'motion/react';
import { FlipHorizontal2, FlipVertical2, RotateCcw, RotateCcwSquare, RotateCwSquare } from 'lucide-react';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { Button, Slider } from '@/components/ui';
import { useFrame } from '../context';
import { aspectRatio, ASPECTS, fitAspect, flipDoc, rotateDoc } from '../geometry';
import { useEditor } from '../store';
import { FULL_CROP, type AspectKey } from '../types';
import { PanelHeading, sectionMotion } from '../ToolTabs';

function AspectIcon({ ratio, free }: { ratio: number | null; free?: boolean }) {
  const r = ratio ?? 1;
  const w = r >= 1 ? 20 : 20 * r;
  const h = r >= 1 ? 20 / r : 20;
  return (
    <span className="grid size-6 place-items-center">
      <span className={cn('block rounded-[3px] border-[1.5px] border-current', free && 'border-dashed')} style={{ width: w, height: h }} />
    </span>
  );
}

export function CropPanel() {
  const t = useT();
  const { frame, crop } = useFrame();
  const geo = useEditor((s) => s.h.doc.geo);
  const change = useEditor((s) => s.change);
  const pulse = useEditor((s) => s.pulse);

  const pickAspect = (key: AspectKey) =>
    change((d) => {
      const ratio = aspectRatio(key, frame.w, frame.h);
      return { ...d, geo: { ...d.geo, aspect: key, crop: ratio ? fitAspect(d.geo.crop, ratio, frame.w, frame.h) : d.geo.crop } };
    });

  const label = (k: AspectKey) => (k === 'free' ? t('photo.crop.free') : k === 'original' ? t('photo.crop.original') : k);

  return (
    <div className="flex flex-col gap-6">
      <motion.section {...sectionMotion(0)}>
        <PanelHeading>{t('photo.crop.aspect')}</PanelHeading>
        <div className="grid grid-cols-4 gap-1.5">
          {ASPECTS.map((a) => {
            const on = geo.aspect === a.key;
            return (
              <button
                key={a.key}
                type="button"
                onClick={() => pickAspect(a.key)}
                className={cn('relative flex flex-col items-center gap-1 rounded-xl px-1 py-2 text-[11px] font-medium transition-colors', on ? 'text-fg' : 'text-dim hover:bg-surface-2 hover:text-fg')}
              >
                {on && <motion.span layoutId="aspect-pill" className="absolute inset-0 rounded-xl bg-surface-3 ring-1 ring-[color-mix(in_oklab,var(--accent)_55%,transparent)]" transition={{ type: 'spring', stiffness: 520, damping: 36 }} />}
                <span className="relative">
                  <AspectIcon ratio={a.key === 'original' ? frame.w / frame.h : a.ratio} free={a.key === 'free'} />
                </span>
                <span className="relative truncate">{label(a.key)}</span>
              </button>
            );
          })}
        </div>
      </motion.section>

      <motion.section {...sectionMotion(1)}>
        <PanelHeading>{t('photo.crop.rotate')}</PanelHeading>
        <div className="grid grid-cols-4 gap-1.5">
          {(
            [
              { label: t('photo.crop.rotateLeft'), icon: <RotateCcwSquare />, run: () => (change((d) => rotateDoc(d, -1, frame.w, frame.h)), pulse('rotate', -1)) },
              { label: t('photo.crop.rotateRight'), icon: <RotateCwSquare />, run: () => (change((d) => rotateDoc(d, 1, frame.w, frame.h)), pulse('rotate', 1)) },
              { label: t('photo.crop.flipH'), icon: <FlipHorizontal2 />, run: () => (change((d) => flipDoc(d, 'h', frame.w, frame.h)), pulse('flipH')) },
              { label: t('photo.crop.flipV'), icon: <FlipVertical2 />, run: () => (change((d) => flipDoc(d, 'v', frame.w, frame.h)), pulse('flipV')) },
            ] as const
          ).map((b) => (
            <motion.button
              key={b.label}
              type="button"
              title={b.label}
              aria-label={b.label}
              whileTap={{ scale: 0.9 }}
              onClick={b.run}
              className="grid h-12 place-items-center rounded-xl border border-line bg-surface text-dim transition-colors hover:bg-surface-2 hover:text-fg [&>svg]:size-5"
            >
              {b.icon}
            </motion.button>
          ))}
        </div>
      </motion.section>

      <motion.section {...sectionMotion(2)}>
        <Slider
          label={t('photo.crop.straighten')}
          value={geo.straighten}
          min={-45}
          max={45}
          step={0.5}
          center={0}
          format={(v) => `${v > 0 ? '+' : ''}${v}°`}
          onChange={(v) => change((d) => ({ ...d, geo: { ...d.geo, straighten: v } }), 'geo.straighten')}
        />
        <div className="mt-1 flex justify-between px-0.5">
          {Array.from({ length: 19 }, (_, i) => (
            <span key={i} className={cn('w-px rounded-full bg-line-strong', i % 3 === 0 ? 'h-2.5' : 'h-1.5', i === 9 && 'bg-accent')} />
          ))}
        </div>
      </motion.section>

      <motion.section {...sectionMotion(3)} className="flex items-center justify-between gap-3 rounded-2xl border border-line bg-surface px-4 py-3">
        <div>
          <div className="text-[11px] font-semibold tracking-wider text-faint uppercase">{t('photo.crop.output')}</div>
          <motion.div key={`${crop.w}x${crop.h}`} initial={{ opacity: 0.4, y: 3 }} animate={{ opacity: 1, y: 0 }} className="font-display text-lg font-semibold tabular-nums">
            {crop.w} × {crop.h}
          </motion.div>
        </div>
        <Button
          size="sm"
          variant="ghost"
          icon={<RotateCcw className="size-3.5" />}
          disabled={geo.straighten === 0 && geo.aspect === 'free' && geo.crop.x === 0 && geo.crop.y === 0 && geo.crop.w === 1 && geo.crop.h === 1}
          onClick={() => change((d) => ({ ...d, geo: { ...d.geo, crop: { ...FULL_CROP }, straighten: 0, aspect: 'free' } }))}
        >
          {t('photo.crop.reset')}
        </Button>
      </motion.section>
    </div>
  );
}
