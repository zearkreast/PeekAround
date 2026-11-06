import { useEffect, useState, type ReactNode } from 'react';
import { useLocation, useNavigate, useParams } from 'react-router';
import { useQuery } from '@tanstack/react-query';
import { AnimatePresence, motion } from 'motion/react';
import { ArrowLeft, Crop, Download, Film, Gauge, Redo2, RotateCcw, Scissors, SlidersHorizontal, Type, Undo2 } from 'lucide-react';
import type { MediaDetails } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useT, type MessageKey } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatDuration } from '@/lib/format';
import { folderHref, withView } from '@/lib/routes';
import { Badge, Button, EmptyState, IconButton, Spinner } from '@/components/ui';
import { ExportPanel, useVideoExport } from './ExportPanel';
import { JobProgressCard } from './JobProgress';
import { AdjustPanel, AudioPanel, CropPanel, TrimPanel } from './panels';
import { Stage } from './Stage';
import { useEditor, usePlayhead, type Tool } from './store';
import { TextPanel } from './TextPanel';
import { Timeline, Transport } from './Timeline';
import { usePlayback, type Playback } from './usePlayback';

const TOOLS: { id: Tool; icon: ReactNode; label: MessageKey }[] = [
  { id: 'trim', icon: <Scissors />, label: 'video.tool.trim' },
  { id: 'crop', icon: <Crop />, label: 'video.tool.crop' },
  { id: 'adjust', icon: <SlidersHorizontal />, label: 'video.tool.adjust' },
  { id: 'text', icon: <Type />, label: 'video.tool.text' },
  { id: 'audio', icon: <Gauge />, label: 'video.tool.audio' },
  { id: 'export', icon: <Download />, label: 'video.tool.export' },
];

export default function VideoEditorPage() {
  const t = useT();
  const { id } = useParams<{ id: string }>();
  const backend = useBackend();
  const nav = useNavigate();
  const q = useQuery({ queryKey: ['media', backend.id, id], queryFn: () => backend.media(id!), enabled: !!id, staleTime: 30_000 });

  if (q.isLoading)
    return (
      <div className="grid h-dvh place-items-center">
        <Spinner className="size-7" />
      </div>
    );
  if (!q.data || q.data.kind !== 'video')
    return (
      <div className="grid h-dvh place-items-center">
        <EmptyState
          icon={<Film />}
          title={t('video.notFound')}
          body={q.error ? (q.error as Error).message : undefined}
          action={
            <Button variant="primary" icon={<ArrowLeft className="size-4" />} onClick={() => nav('/')}>
              {t('common.back')}
            </Button>
          }
        />
      </div>
    );
  return <Editor key={q.data.id} item={q.data} />;
}

function Editor({ item }: { item: MediaDetails }) {
  const t = useT();
  const info = useInfo();
  const [ready] = useState(() => {
    useEditor.getState().init(item.id, { w: item.width || 1280, h: item.height || 720 }, item.duration || 1, !!item.video?.audioCodec);
    return true;
  });
  const pb = usePlayback(item);
  const exp = useVideoExport(item);
  const tool = useEditor((s) => s.tool);
  const setTool = useEditor((s) => s.setTool);
  const disabled = info?.caps.readonly ? t('settings.readonly') : info && !info.caps.videoJobs ? t('video.noFfmpeg') : null;
  const exportNow = () => {
    if (!disabled && !exp.run.busy) exp.start();
  };

  useMetadataFallback(item, pb);
  useEditorKeys(pb, tool);

  if (!ready) return null;
  return (
    <div className="flex h-dvh flex-col overflow-hidden bg-bg" data-testid="video-editor">
      <EditorTopBar item={item} onExport={exportNow} busy={exp.run.busy} disabled={disabled} />
      <div className="flex min-h-0 flex-1 flex-col lg:flex-row">
        <div className="flex min-h-[46vh] min-w-0 flex-1 flex-col">
          <Stage item={item} pb={pb} />
          <div className="shrink-0 border-t border-line bg-elev/60 backdrop-blur-xl">
            <Transport pb={pb} />
            <Timeline item={item} pb={pb} />
          </div>
        </div>
        <aside className="flex max-h-[54vh] min-h-0 shrink-0 flex-col border-t border-line bg-elev lg:max-h-none lg:w-[372px] lg:border-t-0 lg:border-l">
          <nav className="flex shrink-0 gap-0.5 border-b border-line p-1.5" role="tablist">
            {TOOLS.map((x) => (
              <button
                key={x.id}
                type="button"
                role="tab"
                aria-selected={tool === x.id}
                onClick={() => setTool(x.id)}
                className={cn(
                  'relative flex min-w-0 flex-1 flex-col items-center gap-1 rounded-xl px-1 py-2 text-[11px] font-medium transition-colors [&>svg]:size-[18px]',
                  tool === x.id ? 'text-fg' : 'text-faint hover:text-dim',
                )}
                data-testid={`tool-${x.id}`}
              >
                {tool === x.id && <motion.span layoutId="ve-tool" className="absolute inset-0 rounded-xl bg-surface-3" transition={{ type: 'spring', stiffness: 500, damping: 38 }} />}
                <span className={cn('relative [&>svg]:size-[18px]', tool === x.id && 'text-accent')}>{x.icon}</span>
                <span className="relative max-w-full truncate">{t(x.label)}</span>
              </button>
            ))}
          </nav>
          <div className="min-h-0 flex-1 overflow-y-auto">
            <AnimatePresence mode="wait" initial={false}>
              <motion.div key={tool} initial={{ opacity: 0, x: 14 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -14 }} transition={{ duration: 0.16 }}>
                {tool === 'trim' && <TrimPanel />}
                {tool === 'crop' && <CropPanel />}
                {tool === 'adjust' && <AdjustPanel />}
                {tool === 'text' && <TextPanel />}
                {tool === 'audio' && <AudioPanel />}
                {tool === 'export' && <ExportPanel item={item} onExport={exportNow} busy={exp.run.busy} disabled={disabled} />}
              </motion.div>
            </AnimatePresence>
          </div>
        </aside>
      </div>
      <JobProgressCard run={exp.run} kind="video" title={item.name} />
    </div>
  );
}

function EditorTopBar({ item, onExport, busy, disabled }: { item: MediaDetails; onExport: () => void; busy: boolean; disabled: string | null }) {
  const t = useT();
  const nav = useNavigate();
  const loc = useLocation();
  const canUndo = useEditor((s) => s.past.length > 0 || s.s !== s.snap);
  const canRedo = useEditor((s) => s.future.length > 0);
  const undo = useEditor((s) => s.undo);
  const redo = useEditor((s) => s.redo);
  const reset = useEditor((s) => s.reset);
  const back = () => {
    if (loc.key !== 'default') nav(-1);
    else nav(`${folderHref(item.folderId, item.dir)}${withView('', item.id)}`);
  };
  return (
    <header className="app-drag wco-r relative z-20 flex h-14 shrink-0 items-center gap-2 border-b border-line bg-bg/80 pl-2 [--wco-base:0.5rem] backdrop-blur-xl sm:pl-3 sm:[--wco-base:0.75rem]">
      <IconButton label={t('common.back')} onClick={back}>
        <ArrowLeft />
      </IconButton>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <h1 className="truncate font-display text-[15px] font-semibold">{item.name}</h1>
          <span className="hidden shrink-0 sm:inline">
            <Badge>{t('video.editsCopy')}</Badge>
          </span>
        </div>
        <div className="truncate text-[11px] text-faint tabular-nums">
          {item.width}×{item.height} · {formatDuration(item.duration)}
          {item.video?.fps ? ` · ${Math.round(item.video.fps)} fps` : ''}
        </div>
      </div>
      <IconButton label={`${t('common.undo')} (Ctrl+Z)`} onClick={undo} disabled={!canUndo}>
        <Undo2 />
      </IconButton>
      <IconButton label={`${t('common.redo')} (Ctrl+Shift+Z)`} onClick={redo} disabled={!canRedo}>
        <Redo2 />
      </IconButton>
      <IconButton label={t('common.reset')} onClick={reset} className="hidden sm:inline-flex">
        <RotateCcw />
      </IconButton>
      <Button variant="primary" icon={<Download className="size-4" />} onClick={onExport} loading={busy} disabled={!!disabled} title={disabled ?? undefined} aria-label={t('video.export')} className="ml-1" data-testid="export-top">
        <span className="hidden sm:inline">{t('video.export')}</span>
      </Button>
    </header>
  );
}

/** Some media rows have no dimensions/duration yet — take them from the decoded video. */
function useMetadataFallback(item: MediaDetails, pb: Playback) {
  useEffect(() => {
    if (item.width && item.height && item.duration) return;
    const v = pb.videoRef.current;
    if (!v) return;
    const onMeta = () => {
      const st = useEditor.getState();
      const patch: Partial<ReturnType<typeof useEditor.getState>> = {};
      if (!item.width || !item.height) patch.src = { w: v.videoWidth || 1280, h: v.videoHeight || 720 };
      if (!item.duration && Number.isFinite(v.duration)) {
        patch.duration = v.duration;
        useEditor.setState(patch);
        st.set({ trim: { start: 0, end: v.duration } });
        return;
      }
      useEditor.setState(patch);
    };
    v.addEventListener('loadedmetadata', onMeta);
    return () => v.removeEventListener('loadedmetadata', onMeta);
  }, [item, pb.videoRef]);
}

function useEditorKeys(pb: Playback, tool: Tool) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement;
      if (/^(input|textarea|select)$/i.test(el.tagName) || el.isContentEditable) return;
      const st = useEditor.getState();
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key.toLowerCase() === 'z') {
        e.preventDefault();
        if (e.shiftKey) st.redo();
        else st.undo();
        return;
      }
      if (mod && e.key.toLowerCase() === 'y') {
        e.preventDefault();
        st.redo();
        return;
      }
      if (mod) return;
      const now = usePlayhead.getState().time;
      switch (e.key) {
        case ' ':
        case 'k':
          e.preventDefault();
          pb.toggle();
          break;
        case 'i':
        case 'I':
          if (now < st.s.trim.end - 0.2) st.set({ trim: { ...st.s.trim, start: now } });
          break;
        case 'o':
        case 'O':
          if (now > st.s.trim.start + 0.2) st.set({ trim: { ...st.s.trim, end: now } });
          break;
        case 'ArrowLeft':
          e.preventDefault();
          if (e.shiftKey) pb.seek(now - 1);
          else pb.step(-1);
          break;
        case 'ArrowRight':
          e.preventDefault();
          if (e.shiftKey) pb.seek(now + 1);
          else pb.step(1);
          break;
        case 'Delete':
        case 'Backspace':
          if (st.selectedText) {
            e.preventDefault();
            st.set((s) => ({ texts: s.texts.filter((l) => l.id !== st.selectedText) }));
            st.select(null);
          }
          break;
        case 'Escape':
          if (st.selectedText) st.select(null);
          else if (tool === 'crop') st.setTool('trim');
          break;
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [pb, tool]);
}
