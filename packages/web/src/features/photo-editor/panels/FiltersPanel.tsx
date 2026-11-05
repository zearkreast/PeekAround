import { useEffect, useRef } from 'react';
import { motion } from 'motion/react';
import { Check } from 'lucide-react';
import { useT, type MessageKey } from '@/i18n';
import { cn } from '@/lib/cn';
import { Slider } from '@/components/ui';
import { effectiveAdjust, FILTERS } from '../adjust';
import { useEnv, useFrame } from '../context';
import { createRenderer, type Renderer } from '../renderer';
import { useEditor } from '../store';
import { PanelHeading } from '../ToolTabs';

const THUMB = 168; // device px

const TILES: { id: string | null; label: MessageKey }[] = [{ id: null, label: 'photo.filter.none' }, ...FILTERS.map((f) => ({ id: f.id, label: f.label }))];

/** One-click looks, each previewed live on the current photo (with your adjustments + crop). */
export function FiltersPanel() {
  const t = useT();
  const env = useEnv();
  const { frame, crop } = useFrame();
  const doc = useEditor((s) => s.h.doc);
  const change = useEditor((s) => s.change);
  const canvases = useRef(new Map<string, HTMLCanvasElement>());
  const rendererRef = useRef<Renderer | null>(null);

  useEffect(() => {
    if (!env.small) return;
    const r = createRenderer({ preserve: true });
    r.setSource(env.small, env.srcW, env.srcH);
    rendererRef.current = r;
    return () => {
      r.dispose();
      rendererRef.current = null;
    };
  }, [env.small, env.srcW, env.srcH]);

  // re-render the thumbnails (debounced) whenever the photo's base look changes
  useEffect(() => {
    const id = setTimeout(() => {
      const r = rendererRef.current;
      if (!r) return;
      const side = Math.min(crop.w, crop.h);
      const k = side / THUMB;
      for (const tile of TILES) {
        const c = canvases.current.get(tile.id ?? 'none');
        if (!c) continue;
        r.render(
          {
            adjust: effectiveAdjust(doc.adjust, { id: tile.id, strength: 100 }),
            srcW: env.srcW,
            srcH: env.srcH,
            rot: doc.geo.rot,
            flipH: doc.geo.flipH,
            flipV: doc.geo.flipV,
            straighten: doc.geo.straighten,
            frameW: frame.w,
            frameH: frame.h,
            ox: crop.x + (crop.w - side) / 2,
            oy: crop.y + (crop.h - side) / 2,
            k,
            clip: crop,
            crop,
            grainCell: Math.max(1, k),
            sharpenStep: Math.max(1, k),
          },
          THUMB,
          THUMB,
        );
        const ctx = c.getContext('2d');
        if (!ctx) continue;
        ctx.clearRect(0, 0, THUMB, THUMB);
        ctx.drawImage(r.canvas, 0, 0);
      }
    }, 90);
    return () => clearTimeout(id);
  }, [doc.adjust, doc.geo, crop.x, crop.y, crop.w, crop.h, frame.w, frame.h, env.srcW, env.srcH, env.small]);

  const active = doc.filter.id;

  return (
    <div className="flex flex-col gap-5">
      <div className="grid grid-cols-3 gap-2.5">
        {TILES.map((tile, i) => {
          const on = active === tile.id;
          return (
            <motion.button
              key={tile.id ?? 'none'}
              type="button"
              initial={{ opacity: 0, scale: 0.85, y: 10 }}
              animate={{ opacity: 1, scale: 1, y: 0 }}
              transition={{ type: 'spring', stiffness: 420, damping: 28, delay: i * 0.025 }}
              whileHover={{ y: -3 }}
              whileTap={{ scale: 0.95 }}
              onClick={() => change((d) => ({ ...d, filter: { id: tile.id, strength: d.filter.id === tile.id ? d.filter.strength : 100 } }))}
              className="group flex flex-col items-center gap-1.5"
            >
              <span className={cn('relative block aspect-square w-full overflow-hidden rounded-2xl bg-surface-2 transition-shadow', on && 'ring-accent')}>
                <canvas
                  ref={(el) => {
                    if (el) canvases.current.set(tile.id ?? 'none', el);
                    else canvases.current.delete(tile.id ?? 'none');
                  }}
                  width={THUMB}
                  height={THUMB}
                  className="size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-110"
                />
                {on && (
                  <motion.span
                    layoutId="filter-check"
                    className="absolute top-1.5 right-1.5 grid size-5 place-items-center rounded-full bg-accent-gradient text-white shadow"
                    transition={{ type: 'spring', stiffness: 520, damping: 34 }}
                  >
                    <Check className="size-3" strokeWidth={3} />
                  </motion.span>
                )}
              </span>
              <span className={cn('max-w-full truncate text-[12px]', on ? 'font-semibold text-fg' : 'text-dim')}>{t(tile.label)}</span>
            </motion.button>
          );
        })}
      </div>
      <motion.div initial={false} animate={{ opacity: active ? 1 : 0.4 }} className={cn(!active && 'pointer-events-none')}>
        <PanelHeading>{t('photo.filter.strength')}</PanelHeading>
        <Slider
          value={doc.filter.strength}
          min={0}
          max={100}
          center={100}
          format={(v) => `${v}%`}
          label=""
          onChange={(v) => change((d) => ({ ...d, filter: { ...d.filter, strength: v } }), 'filter.strength')}
        />
      </motion.div>
      <p className="text-xs text-faint">{t('photo.filter.hint')}</p>
    </div>
  );
}
