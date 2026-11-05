import { AnimatePresence, Reorder, motion, useDragControls } from 'motion/react';
import { GripVertical, ImageIcon, Plus, Shuffle, Trash2, Video, X } from 'lucide-react';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatDuration } from '@/lib/format';
import { Button } from '@/components/ui';
import { itemDuration, shuffled, type SlideItem } from './show';
import { useSlideshow } from './store';

function NumInput({ value, placeholder, onChange, max, title }: { value: number | null; placeholder: string; onChange: (v: number | null) => void; max: number; title: string }) {
  return (
    <input
      type="number"
      title={title}
      aria-label={title}
      min={0}
      max={max}
      step={0.5}
      value={value ?? ''}
      placeholder={placeholder}
      onPointerDown={(e) => e.stopPropagation()}
      onChange={(e) => {
        const v = e.target.value === '' ? null : Number(e.target.value);
        onChange(v === null || !Number.isFinite(v) ? null : Math.max(0, Math.min(max, v)));
      }}
      className="h-8 w-16 rounded-lg border border-line bg-surface px-2 text-right font-mono text-xs tabular-nums outline-none placeholder:text-faint focus:border-[color-mix(in_oklab,var(--accent)_60%,transparent)]"
    />
  );
}

function Row({ item, index }: { item: SlideItem; index: number }) {
  const t = useT();
  const backend = useBackend();
  const controls = useDragControls();
  const settings = useSlideshow((s) => s.settings);
  const update = useSlideshow((s) => s.update);
  const remove = useSlideshow((s) => s.remove);
  const m = item.media;
  const video = m.kind === 'video';
  const full = m.duration ?? 0;
  return (
    <Reorder.Item
      value={item}
      dragListener={false}
      dragControls={controls}
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.96, transition: { duration: 0.15 } }}
      whileDrag={{ scale: 1.02, boxShadow: '0 20px 50px -12px rgb(0 0 0 / 0.6)', zIndex: 10 }}
      className="relative flex items-center gap-3 rounded-2xl border border-line bg-elev p-2 pr-3"
      data-testid="slide-row"
    >
      <button
        type="button"
        aria-label={t('slide.dragToReorder')}
        className="grid h-12 w-6 shrink-0 cursor-grab touch-none place-items-center rounded-lg text-faint hover:bg-surface-2 hover:text-fg active:cursor-grabbing"
        onPointerDown={(e) => controls.start(e)}
        data-testid="drag-handle"
      >
        <GripVertical className="size-4" />
      </button>
      <span className="w-6 shrink-0 text-center text-xs font-semibold text-faint tabular-nums">{index + 1}</span>
      <div className="relative h-12 w-[72px] shrink-0 overflow-hidden rounded-xl bg-[var(--tile-bg)]">
        <img src={backend.thumbUrl(m.id, 320)} alt="" loading="lazy" draggable={false} className="size-full object-cover" />
        {video && <Video className="absolute bottom-1 left-1 size-3.5 text-white drop-shadow" />}
      </div>
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium">{m.name}</div>
        <div className="flex items-center gap-1.5 text-xs text-faint tabular-nums">
          {video ? <Video className="size-3" /> : <ImageIcon className="size-3" />}
          {formatDuration(itemDuration(item, settings))}
          {video && full ? <span>· {t('slide.ofFull', { d: formatDuration(full) })}</span> : null}
        </div>
      </div>
      {video ? (
        <div className="hidden items-center gap-1 sm:flex">
          <NumInput
            title={t('slide.clipStart')}
            value={item.trim?.start ?? null}
            placeholder="0"
            max={Math.max(0, full - 0.5)}
            onChange={(v) => update(item.key, { trim: v === null && !item.trim?.end ? null : { start: v ?? 0, end: item.trim?.end ?? full } })}
          />
          <span className="text-faint">–</span>
          <NumInput
            title={t('slide.clipEnd')}
            value={item.trim?.end ?? null}
            placeholder={full ? full.toFixed(1) : ''}
            max={full || 3600}
            onChange={(v) => update(item.key, { trim: v === null ? (item.trim?.start ? { start: item.trim.start, end: full } : null) : { start: item.trim?.start ?? 0, end: Math.max((item.trim?.start ?? 0) + 0.5, v) } })}
          />
        </div>
      ) : (
        <div className="flex items-center gap-1.5">
          <NumInput title={t('slide.thisDuration')} value={item.duration} placeholder={String(settings.duration)} max={60} onChange={(v) => update(item.key, { duration: v && v >= 0.5 ? v : null })} />
          <span className="text-xs text-faint">{t('slide.sec')}</span>
        </div>
      )}
      <button type="button" aria-label={t('common.remove')} onClick={() => remove(item.key)} className="grid size-8 shrink-0 place-items-center rounded-lg text-faint hover:bg-danger/10 hover:text-danger" data-testid="remove-slide">
        <X className="size-4" />
      </button>
    </Reorder.Item>
  );
}

export function ItemList({ onAdd }: { onAdd: () => void }) {
  const t = useT();
  const items = useSlideshow((s) => s.items);
  const setItems = useSlideshow((s) => s.setItems);
  const clear = useSlideshow((s) => s.clear);
  return (
    <section>
      <div className="mb-3 flex items-center gap-2">
        <h2 className="font-display text-lg font-semibold">{t('slide.order')}</h2>
        <span className="rounded-full bg-surface-2 px-2 py-0.5 text-xs text-faint tabular-nums">{items.length}</span>
        <div className="flex-1" />
        {items.length > 1 && (
          <Button size="sm" variant="ghost" icon={<Shuffle className="size-4" />} onClick={() => setItems(shuffled(items))}>
            {t('slide.shuffleNow')}
          </Button>
        )}
        {items.length > 0 && (
          <Button size="sm" variant="ghost" icon={<Trash2 className="size-4" />} onClick={clear}>
            {t('slide.clearAll')}
          </Button>
        )}
        <Button size="sm" variant="secondary" icon={<Plus className="size-4" />} onClick={onAdd} data-testid="add-media">
          {t('slide.addMedia')}
        </Button>
      </div>
      <Reorder.Group axis="y" values={items} onReorder={setItems} className="flex flex-col gap-2" layoutScroll>
        <AnimatePresence initial={false}>
          {items.map((it, i) => (
            <Row key={it.key} item={it} index={i} />
          ))}
        </AnimatePresence>
      </Reorder.Group>
      {items.length > 0 && (
        <motion.button
          type="button"
          onClick={onAdd}
          whileHover={{ scale: 1.005 }}
          className={cn('mt-2 flex w-full items-center justify-center gap-2 rounded-2xl border border-dashed border-line-strong py-3 text-sm text-faint transition-colors hover:border-[var(--accent)] hover:text-fg')}
        >
          <Plus className="size-4" />
          {t('slide.addMore')}
        </motion.button>
      )}
    </section>
  );
}
