import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { AnimatePresence, motion } from 'motion/react';
import { CheckSquare, MoreHorizontal, X } from 'lucide-react';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { BACK, useBackHandler } from '@/lib/backStack';
import { useIsMobile } from '@/lib/useMedia';
import { useSelection } from '@/store/selection';
import { BottomSheet, IconButton, SheetItem } from '@/components/ui';
import { iconOf, labelOf, shortLabelOf, useMediaActions } from './actions';
import type { GridItems } from './grid/source';

async function selectAll(source: GridItems | null) {
  if (!source || !source.total) return;
  const items = await source.itemsInRange(0, source.total - 1);
  useSelection.getState().set(items.filter((i) => i.kind !== 'archive'));
}

/**
 * Selection UI. Desktop: floating action pill. Phones: contextual top header
 * (✕, "n seçildi", select all) + bottom action bar with labelled buttons and a "More" sheet.
 * Every action comes from the action registry (actions.tsx). Also owns the selection shortcuts.
 */
export function SelectionBar({ source }: { source: GridItems | null }) {
  const t = useT();
  const mobile = useIsMobile();
  const items = useSelection((s) => s.items);
  const clear = useSelection((s) => s.clear);
  const a = useMediaActions('selection');
  const selected = [...items.values()];
  const n = selected.length;
  const [more, setMore] = useState(false);

  useBackHandler(n > 0, clear, BACK.selection, 'selection');

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (/input|textarea|select/i.test((e.target as HTMLElement).tagName) || document.querySelector('[role=dialog]') || new URLSearchParams(location.search).get('view')) return;
      if (e.key === 'Escape') useSelection.getState().clear();
      else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') {
        e.preventDefault();
        void selectAll(source);
      } else if (e.key === 'F2') {
        // rename: the one selected item, else the focused tile
        const sel = useSelection.getState().selected();
        const idx = Number((document.activeElement as HTMLElement | null)?.closest?.('[data-index]')?.getAttribute('data-index') ?? -1);
        const target = sel.length === 1 ? sel : !sel.length && idx >= 0 && source?.getAt(idx) ? [source.getAt(idx)!] : [];
        const ren = target.length ? a.for(target, 'menu').find((x) => x.id === 'rename') : undefined;
        if (ren) {
          e.preventDefault();
          void a.run(ren, target);
        }
      } else if (e.key === 'Delete' || e.key === 'Backspace') {
        // the selection, else the focused tile (Delete only — Backspace alone is too easy to hit)
        let list = useSelection.getState().selected();
        if (!list.length && e.key === 'Delete') {
          const idx = Number((document.activeElement as HTMLElement | null)?.closest?.('[data-index]')?.getAttribute('data-index') ?? -1);
          const it = idx >= 0 ? source?.getAt(idx) : undefined;
          if (it) list = [it];
        }
        const del = list.length ? a.for(list).find((x) => x.id === 'delete') : undefined;
        if (del) {
          e.preventDefault();
          void a.run(del, list);
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [source, a]);

  // selection is per page
  useEffect(() => () => useSelection.getState().clear(), []);

  const list = n ? a.for(selected) : [];

  if (mobile) {
    const primary = list.filter((x) => x.primary).slice(0, 4);
    const rest = list.filter((x) => !primary.includes(x));
    return createPortal(
      <>
        <AnimatePresence>
          {n > 0 && (
            <motion.header
              key="sel-top"
              initial={{ y: '-100%' }}
              animate={{ y: 0 }}
              exit={{ y: '-100%' }}
              transition={{ type: 'spring', stiffness: 500, damping: 42 }}
              className="fixed inset-x-0 top-0 z-[55] flex items-center gap-1 border-b border-line bg-elev px-2 pt-[env(safe-area-inset-top)] shadow-soft"
              style={{ height: 'calc(var(--topbar-h) + env(safe-area-inset-top, 0px))' }}
            >
              <IconButton label={t('sel.clear')} onClick={clear}>
                <X />
              </IconButton>
              <span className="ml-1 flex-1 text-[17px] font-semibold tabular-nums">{t('mobile.selectedCount', { n })}</span>
              <button type="button" onClick={() => void selectAll(source)} className="touch-target flex h-11 items-center gap-2 rounded-xl px-3 text-sm font-medium text-accent active:bg-surface-2">
                <CheckSquare className="size-5" />
                {t('sel.all')}
              </button>
            </motion.header>
          )}
        </AnimatePresence>
        <AnimatePresence>
          {n > 0 && (
            <motion.nav
              key="sel-bottom"
              initial={{ y: '100%' }}
              animate={{ y: 0 }}
              exit={{ y: '100%' }}
              transition={{ type: 'spring', stiffness: 500, damping: 42 }}
              className="fixed inset-x-0 bottom-0 z-[55] border-t border-line bg-elev pb-[env(safe-area-inset-bottom)] shadow-soft"
            >
              <div className="no-scrollbar flex overflow-x-auto px-1">
                {[...primary, ...(rest.length ? [null] : [])].map((act) =>
                  act ? (
                    <button
                      key={act.id}
                      type="button"
                      onClick={() => void a.run(act, selected)}
                      className={cn(
                        'flex min-h-[60px] min-w-[72px] flex-1 flex-col items-center justify-center gap-1 rounded-xl px-2 text-[12px] font-medium active:bg-surface-2 [&_svg]:size-[22px]',
                        act.destructive ? 'text-danger' : act.active?.(selected, a.ctx) ? 'text-[var(--accent-2)]' : 'text-fg',
                      )}
                    >
                      {iconOf(act, selected, a.ctx)}
                      <span className="max-w-[80px] truncate">{shortLabelOf(act, selected, a.ctx)}</span>
                    </button>
                  ) : (
                    <button
                      key="more"
                      type="button"
                      onClick={() => setMore(true)}
                      className="flex min-h-[60px] min-w-[72px] flex-1 flex-col items-center justify-center gap-1 rounded-xl px-2 text-[12px] font-medium text-fg active:bg-surface-2"
                    >
                      <MoreHorizontal className="size-[22px]" />
                      {t('mobile.more')}
                    </button>
                  ),
                )}
              </div>
            </motion.nav>
          )}
        </AnimatePresence>
        <BottomSheet open={more && n > 0} onClose={() => setMore(false)} title={t('mobile.selectedCount', { n })}>
          {rest.map((act) => (
            <SheetItem
              key={act.id}
              icon={iconOf(act, selected, a.ctx)}
              danger={act.destructive}
              onClick={() => {
                setMore(false);
                void a.run(act, selected);
              }}
            >
              {labelOf(act, selected, a.ctx)}
            </SheetItem>
          ))}
        </BottomSheet>
      </>,
      document.body,
    );
  }

  return (
    <AnimatePresence>
      {n > 0 && (
        <motion.div
          initial={{ y: 80, opacity: 0, scale: 0.9 }}
          animate={{ y: 0, opacity: 1, scale: 1 }}
          exit={{ y: 80, opacity: 0, scale: 0.9 }}
          transition={{ type: 'spring', stiffness: 420, damping: 32 }}
          className="glass fixed bottom-6 left-1/2 z-50 flex max-w-[calc(100vw-24px)] -translate-x-1/2 items-center gap-1 rounded-2xl border border-line p-1.5 pl-4 shadow-soft"
        >
          <motion.span key={n} initial={{ scale: 1.3, opacity: 0.4 }} animate={{ scale: 1, opacity: 1 }} className="mr-2 text-sm font-semibold whitespace-nowrap tabular-nums">
            {t('common.selected', { n })}
          </motion.span>
          <IconButton label={t('sel.all')} onClick={() => void selectAll(source)}>
            <CheckSquare />
          </IconButton>
          <div className="mx-1 h-6 w-px bg-line" />
          {list.map((act, i) => (
            <span key={act.id} className="contents">
              {i > 0 && act.group !== list[i - 1].group && act.group === 'danger' && <div className="mx-1 h-6 w-px bg-line" />}
              <IconButton
                label={labelOf(act, selected, a.ctx)}
                active={act.active?.(selected, a.ctx)}
                onClick={() => void a.run(act, selected)}
                className={act.destructive ? 'hover:!text-danger' : undefined}
              >
                {iconOf(act, selected, a.ctx)}
              </IconButton>
            </span>
          ))}
          <div className="mx-1 h-6 w-px bg-line" />
          <IconButton label={t('sel.clear')} onClick={clear}>
            <X />
          </IconButton>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
