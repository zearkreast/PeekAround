import { useEffect, useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { AnimatePresence, motion } from 'motion/react';
import { ArrowLeft, Check, ChevronRight, Folder, Music, Pause, Play, Video } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatDuration } from '@/lib/format';
import { LibraryIcon } from '@/lib/icons';
import { useLibraries } from '@/lib/queries';
import { Button, MenuLabel, Modal, Spinner } from '@/components/ui';

interface Loc {
  folderId: string;
  dir: string;
  label: string;
}

const matches = (m: MediaItem, mode: 'visual' | 'audio') => (mode === 'audio' ? m.kind === 'audio' : m.kind === 'image' || m.kind === 'video');

/** Browse libraries → folders and pick photos/videos (multi) or one music track. */
export function MediaPicker({ open, mode, onClose, onPick }: { open: boolean; mode: 'visual' | 'audio'; onClose: () => void; onPick: (items: MediaItem[]) => void }) {
  const t = useT();
  const backend = useBackend();
  const libs = useLibraries();
  const [stack, setStack] = useState<Loc[]>([]);
  const [selected, setSelected] = useState<Map<string, MediaItem>>(new Map());
  const [playingId, setPlayingId] = useState<string | null>(null);
  const audio = useRef<HTMLAudioElement | null>(null);
  const cur = stack[stack.length - 1] ?? null;

  const folder = useQuery({
    queryKey: ['picker', backend.id, cur?.folderId, cur?.dir],
    queryFn: () => backend.browse({ folderId: cur!.folderId, dir: cur!.dir, kind: 'all', limit: 1000 }),
    enabled: open && !!cur,
  });
  const allAudio = useQuery({
    queryKey: ['picker-audio', backend.id, libs.data?.libraries.map((l) => l.id).join(',')],
    enabled: open && mode === 'audio' && !!libs.data,
    queryFn: async () => {
      const found = new Map<string, MediaItem>();
      for (const l of libs.data!.libraries) {
        const r = await backend.browse({ libraryId: l.id, recursive: true, kind: 'all', limit: 2000 }).catch(() => null);
        r?.items.filter((i) => i.kind === 'audio').forEach((i) => found.set(i.id, i));
      }
      return [...found.values()];
    },
  });

  useEffect(() => {
    if (open) return;
    setStack([]);
    setSelected(new Map());
    audio.current?.pause();
    setPlayingId(null);
  }, [open]);
  useEffect(() => () => audio.current?.pause(), []);

  const items = useMemo(() => (folder.data?.items ?? []).filter((m) => matches(m, mode)), [folder.data, mode]);

  const preview = (m: MediaItem) => {
    if (playingId === m.id) {
      audio.current?.pause();
      setPlayingId(null);
      return;
    }
    audio.current?.pause();
    const a = new Audio(backend.fileUrl(m.id));
    audio.current = a;
    a.onended = () => setPlayingId(null);
    void a.play().catch(() => {});
    setPlayingId(m.id);
  };
  const toggle = (m: MediaItem) =>
    setSelected((s) => {
      const n = new Map(s);
      if (n.has(m.id)) n.delete(m.id);
      else n.set(m.id, m);
      return n;
    });
  const finish = (list: MediaItem[]) => {
    onPick(list);
    onClose();
  };

  const audioRow = (m: MediaItem) => (
    <div key={m.id} className="flex items-center gap-3 rounded-xl px-2 py-1.5 hover:bg-surface-2">
      <button
        type="button"
        onClick={() => preview(m)}
        aria-label={playingId === m.id ? t('viewer.pause') : t('viewer.play')}
        className={cn('grid size-9 shrink-0 place-items-center rounded-full transition-colors', playingId === m.id ? 'bg-accent-gradient text-white' : 'bg-surface-2 text-dim hover:text-fg')}
      >
        {playingId === m.id ? <Pause className="size-4 fill-current" /> : <Play className="ml-0.5 size-4 fill-current" />}
      </button>
      <button type="button" className="min-w-0 flex-1 text-left" onClick={() => finish([m])} data-testid="music-option">
        <div className="truncate text-sm font-medium">{m.name}</div>
        <div className="text-xs text-faint tabular-nums">{formatDuration(m.duration)}</div>
      </button>
      <Button size="sm" variant="secondary" onClick={() => finish([m])}>
        {t('slide.useTrack')}
      </Button>
    </div>
  );

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={mode === 'audio' ? t('slide.pickMusic') : t('slide.addMedia')}
      width={mode === 'audio' ? 520 : 780}
      footer={
        mode === 'visual' ? (
          <>
            <span className="mr-auto text-sm text-faint">{t('slide.selectedN', { n: selected.size })}</span>
            {items.length > 0 && (
              <Button variant="ghost" onClick={() => setSelected((s) => new Map([...s, ...items.map((i) => [i.id, i] as const)]))}>
                {t('slide.selectAll')}
              </Button>
            )}
            <Button variant="primary" disabled={!selected.size} onClick={() => finish([...selected.values()])} data-testid="picker-add">
              {t('slide.addN', { n: selected.size })}
            </Button>
          </>
        ) : undefined
      }
    >
      <div className="mb-3 flex items-center gap-2">
        <button type="button" disabled={!stack.length} onClick={() => setStack((s) => s.slice(0, -1))} className="grid size-8 place-items-center rounded-lg text-dim hover:bg-surface-2 disabled:opacity-30">
          <ArrowLeft className="size-4" />
        </button>
        <div className="min-w-0 flex-1 truncate text-sm font-medium">{stack.map((s) => s.label).join(' / ') || t('nav.libraries')}</div>
      </div>
      <div className="h-[min(60vh,480px)] overflow-auto rounded-2xl border border-line bg-surface p-1.5">
        <AnimatePresence mode="wait" initial={false}>
          <motion.div key={`${cur?.folderId ?? ''}/${cur?.dir ?? ''}`} initial={{ opacity: 0, x: 12 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -12 }} transition={{ duration: 0.16 }}>
            {!cur && (
              <>
                {mode === 'audio' && (
                  <>
                    <MenuLabel>{t('slide.foundMusic')}</MenuLabel>
                    {allAudio.isLoading && (
                      <div className="grid h-16 place-items-center">
                        <Spinner />
                      </div>
                    )}
                    {allAudio.data?.map(audioRow)}
                    {allAudio.data && !allAudio.data.length && <p className="px-3 py-2 text-sm text-faint">{t('slide.noMusic')}</p>}
                    <MenuLabel>{t('nav.libraries')}</MenuLabel>
                  </>
                )}
                {libs.data?.libraries.flatMap((l) =>
                  l.folders.map((f) => (
                    <Row
                      key={f.id}
                      icon={
                        <span className="grid size-6 place-items-center rounded-md text-white" style={{ background: l.color }}>
                          <LibraryIcon name={l.icon} className="size-3.5" />
                        </span>
                      }
                      label={l.folders.length > 1 ? `${l.name} · ${f.name}` : l.name}
                      onClick={() => setStack([{ folderId: f.id, dir: '', label: l.folders.length > 1 ? f.name : l.name }])}
                    />
                  )),
                )}
              </>
            )}
            {cur && folder.isLoading && (
              <div className="grid h-40 place-items-center">
                <Spinner />
              </div>
            )}
            {cur && folder.data?.folders.map((f) => <Row key={f.dir} icon={<Folder className="size-4 text-accent" />} label={f.name} onClick={() => setStack((s) => [...s, { folderId: cur.folderId, dir: f.dir, label: f.name }])} />)}
            {cur && mode === 'audio' && items.map(audioRow)}
            {cur && mode === 'visual' && items.length > 0 && (
              <div className="grid grid-cols-3 gap-1.5 p-1 sm:grid-cols-5">
                {items.map((m) => {
                  const on = selected.has(m.id);
                  return (
                    <button
                      key={m.id}
                      type="button"
                      onClick={() => toggle(m)}
                      className={cn('group relative aspect-square overflow-hidden rounded-xl bg-[var(--tile-bg)] transition-transform active:scale-95', on && 'ring-accent')}
                      data-testid="picker-item"
                    >
                      <img src={backend.thumbUrl(m.id, 320)} alt="" loading="lazy" className={cn('size-full object-cover transition-transform duration-300', on ? 'scale-90 rounded-lg' : 'group-hover:scale-105')} />
                      {m.kind === 'video' && (
                        <span className="absolute bottom-1.5 left-1.5 inline-flex items-center gap-1 rounded-md bg-black/60 px-1.5 py-0.5 text-[10px] font-medium text-white">
                          <Video className="size-3" />
                          {formatDuration(m.duration)}
                        </span>
                      )}
                      <span className={cn('absolute top-1.5 right-1.5 grid size-5 place-items-center rounded-full border-2 transition-colors', on ? 'border-transparent bg-accent text-white' : 'border-white/80 bg-black/20')}>
                        {on && <Check className="size-3" strokeWidth={3} />}
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
            {cur && !folder.isLoading && !items.length && !folder.data?.folders.length && (
              <div className="flex flex-col items-center gap-2 py-12 text-sm text-faint">
                {mode === 'audio' ? <Music className="size-6" /> : <Folder className="size-6" />}
                {t('slide.nothingHere')}
              </div>
            )}
          </motion.div>
        </AnimatePresence>
      </div>
    </Modal>
  );
}

function Row({ icon, label, onClick }: { icon: React.ReactNode; label: string; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className="flex w-full items-center gap-2.5 rounded-xl px-3 py-2.5 text-left text-sm hover:bg-surface-2">
      {icon}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <ChevronRight className="size-4 text-faint" />
    </button>
  );
}
