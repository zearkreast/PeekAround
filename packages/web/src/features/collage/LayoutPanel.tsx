import { motion } from 'motion/react';
import { Minus, Plus, Shuffle } from 'lucide-react';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { Button } from '@/components/ui';
import { PanelHeading, sectionMotion } from '@/features/photo-editor/ToolTabs';
import { layoutCells } from './geometry';
import { ASPECTS, aspectRatioOf, designSize, type CollageAspect, type CollageDoc } from './model';
import { useCollage, useCollageDoc } from './store';
import { TEMPLATES, templateById } from './templates';

/** Mini, animated preview of a template for the current cell count and canvas ratio. */
function TemplateCard({ id, doc, on, onClick, index }: { id: string; doc: CollageDoc; on: boolean; onClick: () => void; index: number }) {
  const t = useT();
  const tpl = templateById(id);
  const { W, H } = designSize(doc.aspect);
  const box = 96;
  const k = Math.min(box / W, box / H);
  const cells = layoutCells({ ...doc, template: id, gap: 60, padding: 60, radius: 40 });
  return (
    <motion.button
      type="button"
      onClick={onClick}
      initial={{ opacity: 0, y: 14, scale: 0.92 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={{ type: 'spring', stiffness: 380, damping: 28, delay: index * 0.035 }}
      whileHover="hover"
      whileTap={{ scale: 0.96 }}
      className={cn('relative flex flex-col items-center gap-2 rounded-2xl border p-2.5 transition-colors', on ? 'border-transparent' : 'border-line bg-surface hover:bg-surface-2')}
    >
      {on && <motion.span layoutId="tpl-active" className="absolute inset-0 rounded-2xl bg-[color-mix(in_oklab,var(--accent)_14%,transparent)] ring-2 ring-accent" transition={{ type: 'spring', stiffness: 480, damping: 36 }} />}
      <span className="relative grid h-[100px] w-full place-items-center">
        <motion.span
          className="relative block overflow-hidden rounded-md bg-white/90 dark:bg-white/85"
          style={{ width: W * k, height: H * k }}
          variants={{ hover: { scale: 1.06, rotate: tpl.kind === 'free' ? -2 : 0 } }}
          transition={{ type: 'spring', stiffness: 400, damping: 20 }}
        >
          {cells.map((c, i) => (
            <motion.span
              key={`${c.index}-${doc.count}`}
              className="absolute block"
              initial={{ opacity: 0, scale: 0.4 }}
              animate={{ opacity: 1, scale: 1 }}
              transition={{ type: 'spring', stiffness: 500, damping: 26, delay: 0.05 + i * 0.03 }}
              style={{
                left: c.x * k,
                top: c.y * k,
                width: c.w * k,
                height: c.h * k,
                rotate: c.rot,
                borderRadius: Math.max(1.5, c.radius * k),
                background: c.free ? '#fff' : undefined,
                boxShadow: c.free ? '0 1px 3px rgb(0 0 0 / 0.35)' : undefined,
                zIndex: i,
              }}
            >
              <span
                className="absolute block"
                style={{
                  left: c.inner.x * k,
                  top: c.inner.y * k,
                  width: c.inner.w * k,
                  height: c.inner.h * k,
                  borderRadius: Math.max(1, c.innerRadius * k),
                  background: `linear-gradient(135deg, color-mix(in oklab, var(--accent) ${85 - (i % 4) * 12}%, white), color-mix(in oklab, var(--accent-2) ${70 - (i % 3) * 14}%, white))`,
                }}
              />
            </motion.span>
          ))}
        </motion.span>
      </span>
      <span className={cn('relative text-[12px] font-medium', on ? 'text-fg' : 'text-dim')}>{t(tpl.label)}</span>
    </motion.button>
  );
}

function AspectShape({ ratio }: { ratio: number }) {
  const w = ratio >= 1 ? 22 : 22 * ratio;
  const h = ratio >= 1 ? 22 / ratio : 22;
  return <span className="block rounded-[3px] border-[1.5px] border-current" style={{ width: w, height: h }} />;
}

export function LayoutPanel() {
  const t = useT();
  const doc = useCollageDoc();
  const change = useCollage((s) => s.change);
  const photos = useCollage((s) => s.photos);
  const tpl = templateById(doc.template);

  const setCount = (n: number) => change((d) => ({ ...d, count: Math.min(9, Math.max(2, n)) }), 'count');
  const setAspect = (aspect: CollageAspect) =>
    change((d) => {
      // keep text where it was, proportionally
      const a = designSize(d.aspect);
      const b = designSize(aspect);
      return { ...d, aspect, layers: d.layers.map((l) => ({ ...l, x: (l.x / a.W) * b.W, y: (l.y / a.H) * b.H })) };
    });

  return (
    <div className="flex flex-col gap-6">
      <motion.section {...sectionMotion(0)} className="flex items-center justify-between gap-3 rounded-2xl border border-line bg-surface p-3">
        <div>
          <div className="text-[11px] font-semibold tracking-wider text-faint uppercase">{t('collage.cells')}</div>
          <div className="text-xs text-dim">{t('common.photos', { n: photos.length })}</div>
        </div>
        <div className="flex items-center gap-1">
          <motion.button whileTap={{ scale: 0.85 }} type="button" disabled={doc.count <= 2} onClick={() => setCount(doc.count - 1)} className="grid size-9 place-items-center rounded-xl bg-surface-2 hover:bg-surface-3 disabled:opacity-30">
            <Minus className="size-4" />
          </motion.button>
          <motion.span key={doc.count} initial={{ y: -10, opacity: 0, scale: 1.3 }} animate={{ y: 0, opacity: 1, scale: 1 }} className="w-9 text-center font-display text-2xl font-bold tabular-nums">
            {doc.count}
          </motion.span>
          <motion.button whileTap={{ scale: 0.85 }} type="button" disabled={doc.count >= 9} onClick={() => setCount(doc.count + 1)} className="grid size-9 place-items-center rounded-xl bg-surface-2 hover:bg-surface-3 disabled:opacity-30">
            <Plus className="size-4" />
          </motion.button>
        </div>
      </motion.section>

      <motion.section {...sectionMotion(1)}>
        <PanelHeading
          action={
            tpl.kind === 'free' && (
              <Button size="sm" variant="ghost" icon={<Shuffle className="size-3.5" />} onClick={() => change((d) => ({ ...d, seed: d.seed + 1 }))}>
                {t('collage.shuffle')}
              </Button>
            )
          }
        >
          {t('collage.suggested', { n: doc.count })}
        </PanelHeading>
        <div className="grid grid-cols-2 gap-2">
          {TEMPLATES.map((x, i) => (
            <TemplateCard key={x.id} id={x.id} doc={doc} index={i} on={doc.template === x.id} onClick={() => change((d) => ({ ...d, template: x.id }))} />
          ))}
        </div>
      </motion.section>

      <motion.section {...sectionMotion(2)}>
        <PanelHeading>{t('collage.aspect')}</PanelHeading>
        <div className="grid grid-cols-3 gap-1.5">
          {ASPECTS.map((a) => {
            const on = doc.aspect === a.key;
            return (
              <button
                key={a.key}
                type="button"
                onClick={() => setAspect(a.key)}
                className={cn('relative flex h-16 flex-col items-center justify-center gap-1.5 rounded-xl text-[11px] font-medium transition-colors', on ? 'text-fg' : 'text-dim hover:bg-surface-2 hover:text-fg')}
              >
                {on && <motion.span layoutId="collage-aspect" className="absolute inset-0 rounded-xl bg-surface-3 ring-1 ring-[color-mix(in_oklab,var(--accent)_55%,transparent)]" transition={{ type: 'spring', stiffness: 520, damping: 36 }} />}
                <span className="relative grid h-6 place-items-center">
                  <AspectShape ratio={aspectRatioOf(a.key)} />
                </span>
                <span className="relative">{a.key === 'a4' ? 'A4' : a.label ? `${a.key} · ${t(a.label)}` : a.key}</span>
              </button>
            );
          })}
        </div>
      </motion.section>
    </div>
  );
}
