import { useEffect } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { CheckSquare, Copy, Download, FolderInput, Heart, LayoutGrid, Presentation, Trash2, X } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useInfo } from '@/backend';
import { useT } from '@/i18n';
import { useSelection } from '@/store/selection';
import { IconButton } from '@/components/ui';
import { useMediaActions } from './actions';

/** Floating action pill shown while items are selected. Also owns the selection keyboard shortcuts. */
export function SelectionBar({ items, target }: { items: MediaItem[]; target: { folderId: string; dir: string } | null }) {
  const t = useT();
  const info = useInfo();
  const { ids, clear, set } = useSelection();
  const a = useMediaActions();
  const selected = items.filter((i) => ids.has(i.id));
  const writable = info && !info.caps.readonly;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (/input|textarea|select/i.test((e.target as HTMLElement).tagName) || document.querySelector('[role=dialog]')) return;
      if (e.key === 'Escape') clear();
      else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') {
        e.preventDefault();
        set(items.map((i) => i.id));
      } else if ((e.key === 'Delete' || e.key === 'Backspace') && selected.length && writable) {
        e.preventDefault();
        a.remove(selected);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [items, selected, clear, set, a, writable]);

  // selection is per page
  useEffect(() => () => useSelection.getState().clear(), []);

  return (
    <AnimatePresence>
      {selected.length > 0 && (
        <motion.div
          initial={{ y: 80, opacity: 0, scale: 0.9 }}
          animate={{ y: 0, opacity: 1, scale: 1 }}
          exit={{ y: 80, opacity: 0, scale: 0.9 }}
          transition={{ type: 'spring', stiffness: 420, damping: 32 }}
          className="glass fixed bottom-6 left-1/2 z-50 flex -translate-x-1/2 items-center gap-1 rounded-2xl border border-line p-1.5 pl-4 shadow-soft"
        >
          <motion.span key={selected.length} initial={{ scale: 1.3, opacity: 0.4 }} animate={{ scale: 1, opacity: 1 }} className="mr-2 text-sm font-semibold tabular-nums">
            {t('common.selected', { n: selected.length })}
          </motion.span>
          <IconButton label={t('sel.all')} onClick={() => set(items.map((i) => i.id))}>
            <CheckSquare />
          </IconButton>
          <div className="mx-1 h-6 w-px bg-line" />
          <IconButton label={t('sel.favorite')} onClick={() => a.favorite(selected, !selected.every((s) => s.favorite))}>
            <Heart />
          </IconButton>
          <IconButton label={t('sel.download')} onClick={() => a.download(selected)}>
            <Download />
          </IconButton>
          <IconButton label={t('sel.slideshow')} onClick={() => a.slideshow(selected, target)}>
            <Presentation />
          </IconButton>
          {writable && (
            <>
              <IconButton label={t('sel.collage')} onClick={() => a.collage(selected, target)}>
                <LayoutGrid />
              </IconButton>
              <IconButton label={t('sel.move')} onClick={() => a.move(selected)}>
                <FolderInput />
              </IconButton>
              <IconButton label={t('sel.copy')} onClick={() => a.move(selected, true)}>
                <Copy />
              </IconButton>
              <IconButton label={t('sel.delete')} onClick={() => a.remove(selected)} className="hover:!text-danger">
                <Trash2 />
              </IconButton>
            </>
          )}
          <div className="mx-1 h-6 w-px bg-line" />
          <IconButton label={t('sel.clear')} onClick={clear}>
            <X />
          </IconButton>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
