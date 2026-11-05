import { useEffect, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { ImagePlus, LayoutGrid, Palette, Images, Redo2, RotateCcw, Type, Undo2 } from 'lucide-react';
import { useT } from '@/i18n';
import { useChrome } from '@/store/chrome';
import { useWorkbench } from '@/store/workbench';
import { Button, IconButton } from '@/components/ui';
import { isTyping, isUndoKey } from '@/features/photo-editor/history';
import { TextLayersPanel } from '@/features/photo-editor/panels/TextPanel';
import { ToolTabs } from '@/features/photo-editor/ToolTabs';
import { CollageCanvas } from './CollageCanvas';
import { ExportControl } from './ExportControl';
import { LayoutPanel } from './LayoutPanel';
import { designSize, emptySlot } from './model';
import { PhotoPicker } from './PhotoPicker';
import { PhotosPanel } from './PhotosPanel';
import { StylePanel } from './StylePanel';
import { useCollage, useCollageDoc, type CollageTab } from './store';

function TextTab() {
  const t = useT();
  const doc = useCollageDoc();
  const textId = useCollage((s) => s.textId);
  const selectText = useCollage((s) => s.selectText);
  const change = useCollage((s) => s.change);
  const { W, H } = designSize(doc.aspect);
  return (
    <div className="flex flex-col gap-3">
      <TextLayersPanel
        layers={doc.layers}
        selectedId={textId}
        area={{ x: 0, y: 0, w: W, h: H }}
        onSelect={selectText}
        onChange={(id, patch, key) => change((d) => ({ ...d, layers: d.layers.map((l) => (l.id === id ? { ...l, ...patch } : l)) }), key)}
        onLayers={(layers, key) => change((d) => ({ ...d, layers }), key)}
      />
      <p className="text-xs text-faint">{t('collage.textHint')}</p>
    </div>
  );
}

/** Floating photo cards behind the empty state. */
function EmptyCollage({ onPick }: { onPick: () => void }) {
  const t = useT();
  const cards = [
    { x: -150, y: -30, r: -14, d: 0, g: 'from-violet-500 to-fuchsia-500' },
    { x: 140, y: -50, r: 11, d: 0.4, g: 'from-orange-400 to-pink-500' },
    { x: -110, y: 90, r: 8, d: 0.8, g: 'from-sky-400 to-indigo-500' },
    { x: 120, y: 80, r: -9, d: 1.2, g: 'from-emerald-400 to-teal-500' },
  ];
  return (
    <div className="relative grid min-h-[calc(100dvh-var(--topbar-h))] place-items-center overflow-hidden px-6 py-16">
      <div className="aurora opacity-50" />
      <div className="relative flex flex-col items-center text-center">
        <div className="relative mb-10 h-48 w-80">
          {cards.map((c, i) => (
            <motion.div
              key={i}
              className="absolute top-1/2 left-1/2 h-28 w-24 rounded-xl bg-white p-1.5 pb-5 shadow-[0_18px_40px_-12px_rgb(0_0_0/0.6)]"
              initial={{ opacity: 0, scale: 0.4, x: '-50%', y: '-50%', rotate: 0 }}
              animate={{ opacity: 1, scale: 1, x: `calc(-50% + ${c.x}px)`, y: [`calc(-50% + ${c.y}px)`, `calc(-50% + ${c.y - 10}px)`, `calc(-50% + ${c.y}px)`], rotate: c.r }}
              transition={{
                opacity: { delay: 0.1 + i * 0.08 },
                scale: { type: 'spring', stiffness: 260, damping: 18, delay: 0.1 + i * 0.08 },
                x: { type: 'spring', stiffness: 160, damping: 18, delay: 0.1 + i * 0.08 },
                rotate: { type: 'spring', stiffness: 160, damping: 14, delay: 0.1 + i * 0.08 },
                y: { duration: 4, repeat: Infinity, ease: 'easeInOut', delay: c.d },
              }}
            >
              <div className={`size-full rounded-md bg-gradient-to-br ${c.g}`} />
            </motion.div>
          ))}
          <motion.div
            className="absolute top-1/2 left-1/2 grid size-20 -translate-x-1/2 -translate-y-1/2 place-items-center rounded-3xl bg-accent-gradient text-white shadow-[0_16px_40px_-10px_var(--accent)]"
            initial={{ scale: 0, rotate: -30 }}
            animate={{ scale: 1, rotate: 0 }}
            transition={{ type: 'spring', stiffness: 300, damping: 16, delay: 0.45 }}
          >
            <LayoutGrid className="size-9" />
          </motion.div>
        </div>
        <motion.h1 initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: 0.3 }} className="font-display text-3xl font-bold tracking-tight">
          {t('collage.empty.title')}
        </motion.h1>
        <motion.p initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: 0.38 }} className="mt-3 max-w-md text-sm text-dim">
          {t('collage.empty.body')}
        </motion.p>
        <motion.div initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: 0.46 }} className="mt-8">
          <Button variant="primary" size="lg" icon={<ImagePlus className="size-5" />} onClick={onPick}>
            {t('collage.pick')}
          </Button>
        </motion.div>
      </div>
    </div>
  );
}

export default function CollagePage() {
  const t = useT();
  const setChrome = useChrome((s) => s.set);
  const wbItems = useWorkbench((s) => s.items);
  const wbTarget = useWorkbench((s) => s.target);
  const photos = useCollage((s) => s.photos);
  const target = useCollage((s) => s.target);
  const tab = useCollage((s) => s.tab);
  const setTab = useCollage((s) => s.setTab);
  const canUndo = useCollage((s) => s.h.past.length > 0);
  const canRedo = useCollage((s) => s.h.future.length > 0);
  const undo = useCollage((s) => s.undo);
  const redo = useCollage((s) => s.redo);
  const addPhotos = useCollage((s) => s.addPhotos);
  const [picker, setPicker] = useState<{ slot: number | null } | null>(null);

  useEffect(() => setChrome({ crumbs: [], title: t('nav.collage'), viewControls: false }), [setChrome, t]);

  // start from the photos handed over by the gallery ("Make collage")
  useEffect(() => {
    const key = wbItems.map((i) => i.id).join(',');
    if (key && key !== useCollage.getState().sourceKey) useCollage.getState().start(wbItems, wbTarget, key);
  }, [wbItems, wbTarget]);

  // keyboard: undo/redo, delete, escape
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (document.querySelector('[role=dialog]')) return;
      const st = useCollage.getState();
      const u = isUndoKey(e);
      if (u && !isTyping(e)) {
        e.preventDefault();
        if (u === 'undo') st.undo();
        else st.redo();
        return;
      }
      if (isTyping(e)) return;
      if (e.key === 'Escape') {
        st.selectCell(null);
        st.selectText(null);
      } else if (e.key === 'Delete' || e.key === 'Backspace') {
        if (st.textId) {
          const id = st.textId;
          st.change((d) => ({ ...d, layers: d.layers.filter((l) => l.id !== id) }));
          st.selectText(null);
        } else if (st.cell !== null) {
          const i = st.cell;
          st.change((d) => ({ ...d, slots: d.slots.map((s, j) => (j === i ? emptySlot() : s)) }));
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const pickerEl = (
    <PhotoPicker
      open={!!picker}
      single={picker?.slot !== null && picker?.slot !== undefined}
      start={target ?? wbTarget}
      onClose={() => setPicker(null)}
      onPick={(items) => addPhotos(items, picker?.slot ?? undefined)}
    />
  );

  if (!photos.length) {
    return (
      <>
        <EmptyCollage onPick={() => setPicker({ slot: null })} />
        {pickerEl}
      </>
    );
  }

  const tabs: { id: CollageTab; label: string; icon: React.ReactNode }[] = [
    { id: 'layout', label: t('collage.tab.layout'), icon: <LayoutGrid /> },
    { id: 'style', label: t('collage.tab.style'), icon: <Palette /> },
    { id: 'photos', label: t('collage.tab.photos'), icon: <Images /> },
    { id: 'text', label: t('collage.tab.text'), icon: <Type /> },
  ];

  return (
    <div className="flex flex-col lg:h-[calc(100dvh-var(--topbar-h))] lg:flex-row">
      <div className="flex h-[68dvh] min-h-0 flex-col lg:h-auto lg:flex-1">
        <motion.div initial={{ opacity: 0, y: -8 }} animate={{ opacity: 1, y: 0 }} className="flex items-center gap-2 px-4 pt-3 sm:px-6">
          <div className="flex items-center gap-0.5 rounded-xl border border-line bg-surface p-0.5">
            <IconButton size="sm" label={`${t('common.undo')} (Ctrl+Z)`} disabled={!canUndo} onClick={undo}>
              <Undo2 />
            </IconButton>
            <IconButton size="sm" label={`${t('common.redo')} (Ctrl+Shift+Z)`} disabled={!canRedo} onClick={redo}>
              <Redo2 />
            </IconButton>
            <div className="mx-0.5 h-5 w-px bg-line" />
            <IconButton size="sm" label={t('collage.new')} onClick={() => useCollage.getState().start([], wbTarget, wbItems.map((i) => i.id).join(','))}>
              <RotateCcw />
            </IconButton>
          </div>
          <p className="hidden min-w-0 flex-1 truncate text-sm text-dim sm:block">{t('collage.subtitle')}</p>
          <div className="ml-auto">
            <ExportControl />
          </div>
        </motion.div>
        <CollageCanvas onPick={(slot) => setPicker({ slot })} />
      </div>
      <motion.aside
        initial={{ opacity: 0, x: 30 }}
        animate={{ opacity: 1, x: 0 }}
        transition={{ type: 'spring', stiffness: 300, damping: 32 }}
        className="flex min-h-0 flex-col border-t border-line bg-elev lg:w-[360px] lg:border-t-0 lg:border-l"
      >
        <div className="p-3 pb-2">
          <ToolTabs tabs={tabs} value={tab} onChange={setTab} />
        </div>
        <div className="min-h-0 flex-1 overflow-x-hidden px-4 pt-2 pb-10 lg:overflow-y-auto">
          <AnimatePresence mode="wait" initial={false}>
            <motion.div
              key={tab}
              initial={{ opacity: 0, y: 12 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -8, transition: { duration: 0.12 } }}
              transition={{ type: 'spring', stiffness: 420, damping: 34 }}
            >
              {tab === 'layout' && <LayoutPanel />}
              {tab === 'style' && <StylePanel />}
              {tab === 'photos' && <PhotosPanel onAdd={() => setPicker({ slot: null })} />}
              {tab === 'text' && <TextTab />}
            </motion.div>
          </AnimatePresence>
        </div>
      </motion.aside>
      {pickerEl}
    </div>
  );
}
