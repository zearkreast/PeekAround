import { AnimatePresence, motion } from 'motion/react';
import { ImagePlus, X } from 'lucide-react';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { Button } from '@/components/ui';
import { PanelHeading, sectionMotion } from '@/features/photo-editor/ToolTabs';
import { PHOTO_MIME } from './CollageCanvas';
import { emptySlot } from './model';
import { useCollage, useCollageDoc } from './store';

/** The photo pool: tap to place into the selected (or first empty) cell, drag onto a cell, add more, remove. */
export function PhotosPanel({ onAdd }: { onAdd: () => void }) {
  const t = useT();
  const backend = useBackend();
  const photos = useCollage((s) => s.photos);
  const doc = useCollageDoc();
  const cell = useCollage((s) => s.cell);
  const change = useCollage((s) => s.change);
  const removePhoto = useCollage((s) => s.removePhoto);
  const used = new Set(doc.slots.slice(0, doc.count).map((s) => s.id).filter(Boolean));

  const place = (id: string) => {
    change((d) => {
      const slots = [...d.slots];
      let target = cell !== null && cell < d.count ? cell : slots.findIndex((s, i) => i < d.count && !s.id);
      if (target < 0) target = 0;
      const prev = slots.findIndex((s, i) => i < d.count && s.id === id);
      if (prev >= 0 && prev !== target) slots[prev] = { ...slots[target] };
      slots[target] = emptySlot(id);
      return { ...d, slots };
    });
  };

  return (
    <div className="flex flex-col gap-5">
      <motion.div {...sectionMotion(0)}>
        <Button variant="primary" size="lg" className="w-full" icon={<ImagePlus className="size-5" />} onClick={onAdd}>
          {t('collage.add')}
        </Button>
      </motion.div>
      <motion.section {...sectionMotion(1)}>
        <PanelHeading>{t('common.photos', { n: photos.length })}</PanelHeading>
        <div className="grid grid-cols-3 gap-2">
          <AnimatePresence initial={false}>
            {photos.map((p, i) => {
              const inUse = used.has(p.id);
              return (
                <motion.div
                  key={p.id}
                  layout
                  initial={{ opacity: 0, scale: 0.7 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.6 }}
                  transition={{ type: 'spring', stiffness: 460, damping: 30, delay: Math.min(i, 12) * 0.02 }}
                  className="group relative aspect-square"
                >
                  <button
                    type="button"
                    draggable
                    onDragStart={(e) => {
                      e.dataTransfer.setData(PHOTO_MIME, p.id);
                      e.dataTransfer.effectAllowed = 'copy';
                    }}
                    onClick={() => place(p.id)}
                    className={cn('block size-full overflow-hidden rounded-xl bg-surface-2 transition-[box-shadow,opacity]', inUse ? 'ring-2 ring-[color-mix(in_oklab,var(--accent)_70%,transparent)]' : 'opacity-80 hover:opacity-100')}
                  >
                    <img src={backend.thumbUrl(p.id, 320)} alt={p.name} draggable={false} className="size-full object-cover transition-transform duration-500 group-hover:scale-110" />
                  </button>
                  {inUse && <span className="pointer-events-none absolute bottom-1 left-1 rounded-md bg-black/60 px-1.5 py-0.5 text-[10px] font-medium text-white backdrop-blur">{t('collage.inUse')}</span>}
                  <button
                    type="button"
                    title={t('collage.removePhoto')}
                    aria-label={t('collage.removePhoto')}
                    onClick={() => removePhoto(p.id)}
                    className="absolute top-1 right-1 grid size-6 place-items-center rounded-full bg-black/60 text-white opacity-0 backdrop-blur transition-opacity group-hover:opacity-100 hover:bg-danger"
                  >
                    <X className="size-3.5" />
                  </button>
                </motion.div>
              );
            })}
          </AnimatePresence>
        </div>
      </motion.section>
      <p className="text-xs text-faint">{t('collage.photosHint')}</p>
    </div>
  );
}
