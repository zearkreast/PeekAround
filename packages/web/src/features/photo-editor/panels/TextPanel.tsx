import { AnimatePresence, motion, Reorder } from 'motion/react';
import { Copy, GripVertical, Plus, Trash2, Type } from 'lucide-react';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { createTextLayer, type TextLayer } from '@/lib/text';
import { TextLayerControls } from '@/components/TextLayerControls';
import { Button, IconButton } from '@/components/ui';
import { PanelHeading, sectionMotion } from '../ToolTabs';

export const STICKERS = ['😍', '😂', '🥳', '❤️', '🔥', '✨', '⭐', '🌸', '☀️', '🌈', '🎉', '👍', '😎', '🤩', '🐾', '🎈'];

/**
 * Text layer list + property panel. Shared by the photo editor and the collage;
 * `area` is the output rectangle in layer units (new layers are centred and sized to it).
 */
export function TextLayersPanel({
  layers,
  selectedId,
  area,
  onSelect,
  onChange,
  onLayers,
}: {
  layers: TextLayer[];
  selectedId: string | null;
  area: { x: number; y: number; w: number; h: number };
  onSelect: (id: string | null) => void;
  onChange: (id: string, patch: Partial<TextLayer>, key?: string) => void;
  onLayers: (layers: TextLayer[], key?: string) => void;
}) {
  const t = useT();
  const selected = layers.find((l) => l.id === selectedId) ?? null;
  const short = Math.min(area.w, area.h);
  const offset = (layers.length % 5) * short * 0.04;

  const add = (partial: Partial<TextLayer>) => {
    const l = createTextLayer({
      x: area.x + area.w / 2 + offset,
      y: area.y + area.h / 2 + offset,
      fontSize: Math.round(short * 0.09),
      shadow: { color: 'rgba(0,0,0,0.45)', blur: Math.round(short * 0.012), x: 0, y: Math.round(short * 0.003) },
      text: t('photo.text.default'),
      ...partial,
    });
    onLayers([...layers, l]);
    onSelect(l.id);
  };

  const ordered = [...layers].reverse(); // top-most first

  return (
    <div className="flex flex-col gap-5" data-text-panel>
      <motion.div {...sectionMotion(0)}>
        <Button variant="primary" size="lg" className="w-full" icon={<Plus className="size-5" />} onClick={() => add({})}>
          {t('photo.text.add')}
        </Button>
      </motion.div>

      <motion.section {...sectionMotion(1)}>
        <PanelHeading>{t('photo.text.stickers')}</PanelHeading>
        <div className="grid grid-cols-8 gap-1">
          {STICKERS.map((s) => (
            <motion.button
              key={s}
              type="button"
              whileHover={{ scale: 1.25, rotate: -6 }}
              whileTap={{ scale: 0.85 }}
              transition={{ type: 'spring', stiffness: 600, damping: 18 }}
              onClick={() => add({ text: s, fontSize: Math.round(short * 0.16), shadow: null, fontWeight: 400 })}
              className="grid aspect-square place-items-center rounded-xl text-xl hover:bg-surface-2"
            >
              {s}
            </motion.button>
          ))}
        </div>
      </motion.section>

      <motion.section {...sectionMotion(2)}>
        <PanelHeading>{t('photo.text.layers')}</PanelHeading>
        {layers.length === 0 ? (
          <div className="flex items-start gap-3 rounded-2xl border border-dashed border-line-strong p-4 text-sm text-dim">
            <Type className="mt-0.5 size-4 shrink-0 text-accent" />
            {t('photo.text.empty')}
          </div>
        ) : (
          <Reorder.Group axis="y" values={ordered} onReorder={(next) => onLayers([...next].reverse(), 'layers.order')} className="flex flex-col gap-1.5">
            <AnimatePresence initial={false}>
              {ordered.map((l) => {
                const on = l.id === selectedId;
                return (
                  <Reorder.Item
                    key={l.id}
                    value={l}
                    initial={{ opacity: 0, height: 0 }}
                    animate={{ opacity: 1, height: 'auto' }}
                    exit={{ opacity: 0, height: 0 }}
                    whileDrag={{ scale: 1.03, boxShadow: '0 12px 30px -10px rgb(0 0 0 / 0.6)' }}
                    className="relative"
                  >
                    <div
                      onClick={() => onSelect(l.id)}
                      className={cn(
                        'group flex cursor-pointer items-center gap-2 rounded-xl border px-2 py-1.5 transition-colors',
                        on ? 'border-[color-mix(in_oklab,var(--accent)_55%,transparent)] bg-[color-mix(in_oklab,var(--accent)_12%,transparent)]' : 'border-line bg-surface hover:bg-surface-2',
                      )}
                    >
                      <GripVertical className="size-4 shrink-0 cursor-grab text-faint" aria-label={t('photo.text.dragHint')} />
                      <span
                        className="min-w-0 flex-1 truncate text-[15px]"
                        style={{ fontFamily: `"${l.fontFamily}"`, fontWeight: l.fontWeight, fontStyle: l.italic ? 'italic' : undefined, color: on ? undefined : 'var(--text-dim)' }}
                      >
                        {l.text.split('\n')[0] || '…'}
                      </span>
                      <span className="size-3.5 shrink-0 rounded-full ring-1 ring-white/20" style={{ background: l.color }} />
                      <IconButton
                        size="sm"
                        label={t('photo.text.duplicate')}
                        className="opacity-60 group-hover:opacity-100"
                        onClick={(e) => {
                          e.stopPropagation();
                          const copy = { ...l, id: createTextLayer().id, x: l.x + short * 0.03, y: l.y + short * 0.03 };
                          onLayers([...layers, copy]);
                          onSelect(copy.id);
                        }}
                      >
                        <Copy />
                      </IconButton>
                      <IconButton
                        size="sm"
                        label={t('photo.text.delete')}
                        className="opacity-60 group-hover:opacity-100 hover:!text-danger"
                        onClick={(e) => {
                          e.stopPropagation();
                          onLayers(layers.filter((x) => x.id !== l.id));
                          if (on) onSelect(null);
                        }}
                      >
                        <Trash2 />
                      </IconButton>
                    </div>
                  </Reorder.Item>
                );
              })}
            </AnimatePresence>
          </Reorder.Group>
        )}
      </motion.section>

      <AnimatePresence mode="wait">
        {selected && (
          <motion.section
            key={selected.id}
            initial={{ opacity: 0, y: 16, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8, scale: 0.98 }}
            transition={{ type: 'spring', stiffness: 420, damping: 34 }}
            className="rounded-3xl border border-line bg-elev-2 p-3.5"
          >
            <TextLayerControls
              layer={selected}
              maxSize={Math.max(200, Math.round(Math.max(area.w, area.h) * 0.5))}
              onChange={(patch) => onChange(selected.id, patch, `text.${selected.id}.${Object.keys(patch).join(',')}`)}
            />
          </motion.section>
        )}
      </AnimatePresence>
      <p className="text-xs text-faint">{t('photo.text.hint')}</p>
    </div>
  );
}
