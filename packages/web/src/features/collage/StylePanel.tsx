import { motion } from 'motion/react';
import { Check } from 'lucide-react';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { ColorSwatches, Segmented, Slider } from '@/components/ui';
import { PanelHeading, sectionMotion } from '@/features/photo-editor/ToolTabs';
import { cssGradient, designSize, DESIGN_LONG, GRADIENTS, RESOLUTIONS, SOLID_COLORS, type Background } from './model';
import { useCollage, useCollageDoc } from './store';

export function StylePanel() {
  const t = useT();
  const doc = useCollageDoc();
  const change = useCollage((s) => s.change);
  const setBg = (bg: Background, key?: string) => change((d) => ({ ...d, bg }), key);
  const { W, H } = designSize(doc.aspect);
  const k = doc.longSide / DESIGN_LONG;

  return (
    <div className="flex flex-col gap-6">
      <motion.section {...sectionMotion(0)} className="flex flex-col gap-3">
        <Slider label={t('collage.gap')} value={doc.gap} min={0} max={100} center={24} onChange={(gap) => change((d) => ({ ...d, gap }), 'gap')} />
        <Slider label={t('collage.padding')} value={doc.padding} min={0} max={180} center={24} onChange={(padding) => change((d) => ({ ...d, padding }), 'padding')} />
        <Slider label={t('collage.radius')} value={doc.radius} min={0} max={140} center={12} onChange={(radius) => change((d) => ({ ...d, radius }), 'radius')} />
      </motion.section>

      <motion.section {...sectionMotion(1)}>
        <PanelHeading>{t('collage.background')}</PanelHeading>
        <Segmented
          className="mb-3 w-full [&>button]:flex-1"
          value={doc.bg.kind}
          onChange={(kind) => setBg(kind === 'solid' ? { kind, color: '#ffffff' } : kind === 'gradient' ? { kind, id: GRADIENTS[0].id } : { kind, amount: 40 })}
          options={[
            { value: 'solid', label: t('collage.bg.solid') },
            { value: 'gradient', label: t('collage.bg.gradient') },
            { value: 'blur', label: t('collage.bg.blur') },
          ]}
        />
        <motion.div key={doc.bg.kind} initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ type: 'spring', stiffness: 420, damping: 32 }}>
          {doc.bg.kind === 'solid' && <ColorSwatches value={doc.bg.color} colors={SOLID_COLORS} onChange={(color) => setBg({ kind: 'solid', color }, 'bg.color')} />}
          {doc.bg.kind === 'gradient' && (
            <div className="grid grid-cols-4 gap-2">
              {GRADIENTS.map((g, i) => {
                const on = doc.bg.kind === 'gradient' && doc.bg.id === g.id;
                return (
                  <motion.button
                    key={g.id}
                    type="button"
                    initial={{ opacity: 0, scale: 0.7 }}
                    animate={{ opacity: 1, scale: 1 }}
                    transition={{ type: 'spring', stiffness: 500, damping: 28, delay: i * 0.03 }}
                    whileHover={{ y: -3 }}
                    whileTap={{ scale: 0.92 }}
                    onClick={() => setBg({ kind: 'gradient', id: g.id })}
                    className={cn('relative grid aspect-square place-items-center rounded-xl', on && 'ring-accent')}
                    style={{ background: cssGradient(g) }}
                  >
                    {on && <Check className="size-4 text-white drop-shadow" strokeWidth={3} />}
                  </motion.button>
                );
              })}
            </div>
          )}
          {doc.bg.kind === 'blur' && (
            <Slider label={t('collage.blurAmount')} value={doc.bg.amount} min={8} max={120} center={40} onChange={(amount) => setBg({ kind: 'blur', amount }, 'bg.blur')} />
          )}
        </motion.div>
      </motion.section>

      <motion.section {...sectionMotion(2)}>
        <PanelHeading>{t('collage.resolution')}</PanelHeading>
        <Segmented
          className="w-full [&>button]:flex-1"
          value={String(doc.longSide)}
          onChange={(v) => change((d) => ({ ...d, longSide: Number(v) }))}
          options={RESOLUTIONS.map((r) => ({ value: String(r), label: `${r}px` }))}
        />
        <div className="mt-2 text-xs text-faint tabular-nums">{t('collage.size', { w: Math.round(W * k), h: Math.round(H * k) })}</div>
      </motion.section>
    </div>
  );
}
