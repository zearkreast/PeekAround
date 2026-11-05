import { useEffect, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { ArrowLeft, Check, ChevronRight, Folder, ImageOff } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { LibraryIcon } from '@/lib/icons';
import { useBrowse, useLibraries } from '@/lib/queries';
import { thumbhashUrl } from '@/lib/thumbhash';
import { Button, Modal, Spinner } from '@/components/ui';

interface Loc {
  folderId: string;
  dir: string;
  label: string;
}

/**
 * Browse libraries → folders and pick photos. `single` picks one photo immediately
 * (replace a cell); otherwise multi-select with a confirm button.
 */
export function PhotoPicker({
  open,
  single,
  start,
  onClose,
  onPick,
}: {
  open: boolean;
  single?: boolean;
  start?: { folderId: string; dir: string } | null;
  onClose: () => void;
  onPick: (items: MediaItem[]) => void;
}) {
  const t = useT();
  const backend = useBackend();
  const libs = useLibraries();
  const [stack, setStack] = useState<Loc[]>([]);
  const [sel, setSel] = useState<Map<string, MediaItem>>(new Map());
  const cur = stack[stack.length - 1] ?? null;
  const browse = useBrowse({ folderId: cur?.folderId, dir: cur?.dir, kind: 'image', sort: 'takenAt', order: 'desc', limit: 300 }, open && !!cur);

  // open at the workbench folder when we know it (once per opening)
  const inited = useRef(false);
  useEffect(() => {
    if (!open) {
      inited.current = false;
      return;
    }
    if (inited.current || !libs.data) return;
    inited.current = true;
    setSel(new Map());
    if (start) {
      const all = [...libs.data.libraries.flatMap((l) => l.folders), ...libs.data.temp];
      const f = all.find((x) => x.id === start.folderId);
      if (f) {
        const parts = start.dir ? start.dir.split('/') : [];
        setStack([{ folderId: f.id, dir: '', label: f.name }, ...parts.map((p, i) => ({ folderId: f.id, dir: parts.slice(0, i + 1).join('/'), label: p }))]);
        return;
      }
    }
    setStack([]);
  }, [open, start, libs.data]);

  const toggle = (m: MediaItem) => {
    if (single) {
      onPick([m]);
      onClose();
      return;
    }
    setSel((s) => {
      const n = new Map(s);
      if (n.has(m.id)) n.delete(m.id);
      else n.set(m.id, m);
      return n;
    });
  };

  const images = browse.items.filter((i) => i.kind === 'image');

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={single ? t('collage.picker.replace') : t('collage.picker.title')}
      width={880}
      footer={
        single ? undefined : (
          <>
            <span className="mr-auto text-sm text-dim tabular-nums">{t('common.selected', { n: sel.size })}</span>
            <Button variant="ghost" onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              disabled={!sel.size}
              onClick={() => {
                onPick([...sel.values()]);
                onClose();
              }}
            >
              {t('collage.picker.add', { n: sel.size })}
            </Button>
          </>
        )
      }
    >
      <div className="mb-3 flex items-center gap-2">
        <button type="button" disabled={!stack.length} onClick={() => setStack((s) => s.slice(0, -1))} className="grid size-8 place-items-center rounded-lg text-dim hover:bg-surface-2 disabled:opacity-30">
          <ArrowLeft className="size-4" />
        </button>
        <div className="min-w-0 flex-1 truncate text-sm font-medium">{stack.map((s) => s.label).join(' / ') || t('nav.libraries')}</div>
      </div>
      <div className="h-[56vh] overflow-auto rounded-2xl border border-line bg-surface p-2">
        <AnimatePresence mode="wait" initial={false}>
          <motion.div key={`${cur?.folderId ?? ''}/${cur?.dir ?? ''}`} initial={{ opacity: 0, x: 14 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -14 }} transition={{ duration: 0.16 }}>
            {!cur && (
              <div className="grid gap-1.5 sm:grid-cols-2">
                {libs.data?.libraries.flatMap((l) =>
                  l.folders.map((f) => (
                    <button
                      key={f.id}
                      type="button"
                      onClick={() => setStack([{ folderId: f.id, dir: '', label: l.folders.length > 1 ? `${l.name} · ${f.name}` : l.name }])}
                      className="flex items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm hover:bg-surface-2"
                    >
                      <span className="grid size-8 shrink-0 place-items-center rounded-lg text-white" style={{ background: l.color }}>
                        <LibraryIcon name={l.icon} className="size-4" />
                      </span>
                      <span className="min-w-0 flex-1 truncate">{l.folders.length > 1 ? `${l.name} · ${f.name}` : l.name}</span>
                      <span className="text-xs text-faint tabular-nums">{f.mediaCount}</span>
                      <ChevronRight className="size-4 text-faint" />
                    </button>
                  )),
                )}
                {libs.data?.temp.map((f) => (
                  <button key={f.id} type="button" onClick={() => setStack([{ folderId: f.id, dir: '', label: f.name }])} className="flex items-center gap-3 rounded-xl px-3 py-2.5 text-left text-sm hover:bg-surface-2">
                    <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-surface-3 text-dim">
                      <Folder className="size-4" />
                    </span>
                    <span className="min-w-0 flex-1 truncate">{f.name}</span>
                    <ChevronRight className="size-4 text-faint" />
                  </button>
                ))}
              </div>
            )}
            {cur && browse.isLoading && (
              <div className="grid h-48 place-items-center">
                <Spinner />
              </div>
            )}
            {cur && !!browse.first?.folders.length && (
              <div className="mb-3 flex flex-wrap gap-1.5">
                {browse.first.folders.map((f) => (
                  <button
                    key={f.dir}
                    type="button"
                    onClick={() => setStack((s) => [...s, { folderId: cur.folderId, dir: f.dir, label: f.name }])}
                    className="inline-flex items-center gap-1.5 rounded-full border border-line bg-surface-2 px-3 py-1.5 text-[13px] hover:bg-surface-3"
                  >
                    <Folder className="size-3.5 text-accent" />
                    {f.name}
                    <span className="text-[11px] text-faint">{f.mediaCount}</span>
                  </button>
                ))}
              </div>
            )}
            {cur && !browse.isLoading && images.length === 0 && (
              <div className="flex flex-col items-center gap-2 py-14 text-sm text-faint">
                <ImageOff className="size-7" />
                {t('collage.picker.empty')}
              </div>
            )}
            {cur && images.length > 0 && (
              <div className="grid grid-cols-3 gap-1.5 sm:grid-cols-5">
                {images.map((m, i) => {
                  const on = sel.has(m.id);
                  const ph = thumbhashUrl(m.thumbhash);
                  return (
                    <motion.button
                      key={m.id}
                      type="button"
                      initial={{ opacity: 0, scale: 0.9 }}
                      animate={{ opacity: 1, scale: 1 }}
                      transition={{ type: 'spring', stiffness: 400, damping: 30, delay: Math.min(i, 30) * 0.012 }}
                      onClick={() => toggle(m)}
                      className="group relative aspect-square overflow-hidden rounded-xl bg-surface-2"
                      style={ph ? { backgroundImage: `url(${ph})`, backgroundSize: 'cover' } : undefined}
                    >
                      <img src={backend.thumbUrl(m.id, 320)} alt={m.name} loading="lazy" draggable={false} className={cn('size-full object-cover transition-transform duration-300', on ? 'scale-[0.86] rounded-lg' : 'group-hover:scale-105')} />
                      <span
                        className={cn(
                          'absolute top-1.5 right-1.5 grid size-6 place-items-center rounded-full border-2 transition-all',
                          on ? 'border-transparent bg-accent text-white' : 'border-white/80 bg-black/20 opacity-0 group-hover:opacity-100',
                          single && 'hidden',
                        )}
                      >
                        {on && <Check className="size-3.5" strokeWidth={3} />}
                      </span>
                    </motion.button>
                  );
                })}
              </div>
            )}
          </motion.div>
        </AnimatePresence>
      </div>
    </Modal>
  );
}
