import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useNavigate, useParams } from 'react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { AnimatePresence, motion } from 'motion/react';
import { Brush, Crop, ImageOff, SlidersHorizontal, Sparkles, Type } from 'lucide-react';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { go } from '@/lib/nav';
import { folderHref, withView } from '@/lib/routes';
import { toast } from '@/store/toasts';
import { Button, ConfirmDialog, EmptyState, Spinner } from '@/components/ui';
import { EditorEnvContext, useFrame, type EditorEnv, type SourceImage } from './context';
import { EditorStage } from './EditorStage';
import { EditorTopBar, type SaveFormat, type SaveState } from './EditorTopBar';
import { renderEdit } from './exporter';
import { isTyping, isUndoKey } from './history';
import { AdjustPanel } from './panels/AdjustPanel';
import { CropPanel } from './panels/CropPanel';
import { DrawPanel } from './panels/DrawPanel';
import { FiltersPanel } from './panels/FiltersPanel';
import { TextLayersPanel } from './panels/TextPanel';
import { createRenderer, type Renderer } from './renderer';
import { useEditor } from './store';
import { ToolTabs } from './ToolTabs';
import type { EditorTab } from './types';

let warnedNoGl = false;

async function decode(img: HTMLImageElement): Promise<SourceImage> {
  try {
    return await createImageBitmap(img, { imageOrientation: 'from-image' });
  } catch {
    return img;
  }
}

function smallCopy(src: SourceImage, w: number, h: number, max = 320) {
  const k = Math.min(1, max / Math.max(w, h));
  const c = document.createElement('canvas');
  c.width = Math.max(1, Math.round(w * k));
  c.height = Math.max(1, Math.round(h * k));
  const ctx = c.getContext('2d')!;
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(src, 0, 0, c.width, c.height);
  return c;
}

function TextTab() {
  const { crop } = useFrame();
  const layers = useEditor((s) => s.h.doc.layers);
  const selectedId = useEditor((s) => s.selectedId);
  const select = useEditor((s) => s.select);
  const change = useEditor((s) => s.change);
  return (
    <TextLayersPanel
      layers={layers}
      selectedId={selectedId}
      area={crop}
      onSelect={select}
      onChange={(id, patch, key) => change((d) => ({ ...d, layers: d.layers.map((l) => (l.id === id ? { ...l, ...patch } : l)) }), key)}
      onLayers={(next, key) => change((d) => ({ ...d, layers: next }), key)}
    />
  );
}

function ToolPanel() {
  const t = useT();
  const tab = useEditor((s) => s.tab);
  const setTab = useEditor((s) => s.setTab);
  const tabs: { id: EditorTab; label: string; icon: React.ReactNode }[] = [
    { id: 'adjust', label: t('photo.tab.adjust'), icon: <SlidersHorizontal /> },
    { id: 'filters', label: t('photo.tab.filters'), icon: <Sparkles /> },
    { id: 'crop', label: t('photo.tab.crop'), icon: <Crop /> },
    { id: 'text', label: t('photo.tab.text'), icon: <Type /> },
    { id: 'draw', label: t('photo.tab.draw'), icon: <Brush /> },
  ];
  const order = tabs.findIndex((x) => x.id === tab);
  const [prev, setPrev] = useState(order);
  const dir = order >= prev ? 1 : -1;
  useEffect(() => setPrev(order), [order]);

  return (
    <motion.aside
      initial={{ opacity: 0, x: 40 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ type: 'spring', stiffness: 300, damping: 32, delay: 0.05 }}
      className="relative z-10 flex h-[46dvh] shrink-0 flex-col border-t border-line bg-elev md:h-auto md:w-[356px] md:border-t-0 md:border-l"
    >
      <div className="p-3 pb-2">
        <ToolTabs tabs={tabs} value={tab} onChange={setTab} />
      </div>
      <div className="relative min-h-0 flex-1 overflow-x-hidden overflow-y-auto px-4 pt-2 pb-8">
        <AnimatePresence mode="wait" initial={false} custom={dir}>
          <motion.div
            key={tab}
            custom={dir}
            initial={{ opacity: 0, x: 24 * dir }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -24 * dir, transition: { duration: 0.12 } }}
            transition={{ type: 'spring', stiffness: 420, damping: 36 }}
          >
            {tab === 'adjust' && <AdjustPanel />}
            {tab === 'filters' && <FiltersPanel />}
            {tab === 'crop' && <CropPanel />}
            {tab === 'text' && <TextTab />}
            {tab === 'draw' && <DrawPanel />}
          </motion.div>
        </AnimatePresence>
      </div>
    </motion.aside>
  );
}

export default function PhotoEditorPage() {
  const { id = '' } = useParams();
  const t = useT();
  const backend = useBackend();
  const nav = useNavigate();
  const qc = useQueryClient();
  const media = useQuery({ queryKey: ['media', backend.id, id], queryFn: () => backend.media(id), retry: 1 });
  const item = media.data;

  const [renderer, setRenderer] = useState<Renderer | null>(null);
  const [source, setSource] = useState<SourceImage | null>(null);
  const [small, setSmall] = useState<HTMLCanvasElement | null>(null);
  const [loadError, setLoadError] = useState(false);
  const [, bump] = useState(0);
  const [saving, setSaving] = useState<SaveState | null>(null);
  const [saveSignal, setSaveSignal] = useState(0);
  const [confirmLeave, setConfirmLeave] = useState(false);
  const compare = useEditor((s) => s.compare);
  const dirty = useEditor((s) => s.h.doc !== s.saved);

  // preview renderer lives as long as the page
  useEffect(() => {
    const r = createRenderer();
    setRenderer(r);
    if (r.kind !== 'webgl2' && !warnedNoGl) {
      warnedNoGl = true;
      toast.info(t('photo.noWebgl'));
    }
    return () => r.dispose();
  }, []);

  // fresh document per photo
  useEffect(() => {
    useEditor.getState().reset();
    setSource(null);
    setSmall(null);
    setLoadError(false);
  }, [id]);

  // load the full-size, browser-decodable image
  useEffect(() => {
    if (!item || item.kind !== 'image') return;
    let alive = true;
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.decoding = 'async';
    img.onload = async () => {
      const src = await decode(img);
      if (!alive) return;
      setSource(src);
      setSmall(smallCopy(src, src.width, src.height));
    };
    img.onerror = () => alive && setLoadError(true);
    img.src = backend.displayUrl(item);
    return () => {
      alive = false;
      img.onload = img.onerror = null;
    };
  }, [item, backend]);

  useEffect(() => {
    if (!renderer || !source) return;
    renderer.setSource(source, source.width, source.height);
    bump((x) => x + 1);
  }, [renderer, source]);

  const env: EditorEnv | null = useMemo(
    () =>
      item
        ? {
            item,
            srcW: source?.width ?? item.width ?? 1600,
            srcH: source?.height ?? item.height ?? 1067,
            source,
            small,
            renderer,
          }
        : null,
    [item, source, small, renderer],
  );

  const leave = useCallback(() => {
    const idx = (window.history.state as { idx?: number } | null)?.idx ?? 0;
    if (idx > 0) nav(-1);
    else if (item) nav(folderHref(item.folderId, item.dir) + withView('', item.id));
    else nav('/');
  }, [nav, item]);
  const back = () => (dirty && !saving ? setConfirmLeave(true) : leave());

  const save = async (format: SaveFormat, quality: number) => {
    if (!item || !source || saving) return;
    const doc = useEditor.getState().h.doc;
    setSaving({ progress: 0.02, label: t('photo.rendering') });
    try {
      const blob = await renderEdit(source, source.width, source.height, doc, (p) => setSaving({ progress: p, label: p < 0.8 ? t('photo.rendering') : t('photo.encoding') }));
      setSaving({ progress: 0.9, label: t('common.saving') });
      const res = await backend.saveEdit(blob, { sourceId: item.id, format, quality });
      setSaving({ progress: 1, label: t('common.saved') });
      if (useEditor.getState().h.doc === doc) useEditor.getState().markSaved();
      void qc.invalidateQueries({ queryKey: ['browse'] });
      toast.success(
        t('photo.saved'),
        res.item.name,
        res.id ? { label: t('common.open'), run: () => go(folderHref(res.item.folderId, res.item.dir) + withView('', res.id)) } : undefined,
      );
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setTimeout(() => setSaving(null), 500);
    }
  };

  // keyboard: undo/redo, delete layer, compare (hold \), save (Ctrl+S), zoom
  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (document.querySelector('[role=dialog]')) return;
      const st = useEditor.getState();
      const u = isUndoKey(e);
      if (u && !isTyping(e)) {
        e.preventDefault();
        if (u === 'undo') st.undo();
        else st.redo();
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
        e.preventDefault();
        setSaveSignal((x) => x + 1);
        return;
      }
      if (isTyping(e)) return;
      if ((e.key === 'Delete' || e.key === 'Backspace') && st.selectedId) {
        e.preventDefault();
        const sid = st.selectedId;
        st.change((d) => ({ ...d, layers: d.layers.filter((l) => l.id !== sid) }));
        st.select(null);
      } else if (e.key === 'Escape' && st.selectedId) st.select(null);
      else if (e.key === '\\' && !e.repeat) st.setCompare(true);
      else if (e.key === '0' && st.tab !== 'crop') st.setZoom('fit');
      else if (e.key === '1' && st.tab !== 'crop') st.setZoom(1);
    };
    const up = (e: KeyboardEvent) => {
      if (e.key === '\\') useEditor.getState().setCompare(false);
    };
    const blur = () => useEditor.getState().setCompare(false);
    window.addEventListener('keydown', down);
    window.addEventListener('keyup', up);
    window.addEventListener('blur', blur);
    return () => {
      window.removeEventListener('keydown', down);
      window.removeEventListener('keyup', up);
      window.removeEventListener('blur', blur);
    };
  }, []);

  // warn before closing the tab with unsaved edits
  useEffect(() => {
    if (!dirty) return;
    const fn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener('beforeunload', fn);
    return () => window.removeEventListener('beforeunload', fn);
  }, [dirty]);

  let body: React.ReactNode;
  if (media.isLoading) {
    body = (
      <div className="grid flex-1 place-items-center">
        <Spinner className="size-7" />
      </div>
    );
  } else if (!item || item.kind !== 'image' || loadError) {
    body = (
      <div className="grid flex-1 place-items-center">
        <EmptyState
          icon={<ImageOff />}
          title={!item ? t('photo.notFound') : loadError ? t('photo.loadError') : t('photo.notFound')}
          body={item && item.kind !== 'image' ? t('photo.notImage') : undefined}
          action={<Button onClick={leave}>{t('common.back')}</Button>}
        />
      </div>
    );
  } else {
    body = (
      <div className="flex min-h-0 flex-1 flex-col md:flex-row">
        <div className="relative flex min-h-0 flex-1 flex-col">
          <EditorStage />
          <AnimatePresence>
            {compare && (
              <motion.div
                initial={{ opacity: 0, y: -10, scale: 0.9 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, y: -6, scale: 0.95 }}
                transition={{ type: 'spring', stiffness: 520, damping: 30 }}
                className="glass pointer-events-none absolute top-4 left-1/2 -translate-x-1/2 rounded-full border border-line px-3.5 py-1.5 text-xs font-semibold tracking-wide uppercase"
              >
                {t('photo.original')}
              </motion.div>
            )}
          </AnimatePresence>
          <AnimatePresence>
            {saving && (
              <motion.div
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                className="pointer-events-none absolute inset-0 overflow-hidden"
              >
                <motion.div
                  className="absolute inset-y-0 w-1/3 bg-[linear-gradient(100deg,transparent,color-mix(in_oklab,var(--accent)_22%,transparent),transparent)]"
                  animate={{ x: ['-120%', '320%'] }}
                  transition={{ duration: 1.4, repeat: Infinity, ease: 'easeInOut' }}
                />
              </motion.div>
            )}
          </AnimatePresence>
        </div>
        <ToolPanel />
      </div>
    );
  }

  return createPortal(
    <motion.div
      initial={{ opacity: 0, scale: 0.985 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ type: 'spring', stiffness: 300, damping: 30 }}
      className="fixed inset-0 z-40 flex flex-col overflow-hidden bg-[#060609] text-fg"
    >
      {env ? (
        <EditorEnvContext.Provider value={env}>
          <EditorTopBar onBack={back} onSave={(f, q) => void save(f, q)} saving={saving} saveSignal={saveSignal} />
          {body}
        </EditorEnvContext.Provider>
      ) : (
        body
      )}
      <ConfirmDialog
        open={confirmLeave}
        title={t('photo.unsaved')}
        body={t('photo.unsavedBody')}
        confirmLabel={t('photo.leave')}
        cancelLabel={t('common.cancel')}
        danger
        onConfirm={() => {
          useEditor.getState().markSaved();
          leave();
        }}
        onClose={() => setConfirmLeave(false)}
      />
    </motion.div>,
    document.body,
  );
}
