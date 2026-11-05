import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate, useSearchParams, useLocation as useRouterLocation } from 'react-router';
import { AnimatePresence, motion, type PanInfo } from 'motion/react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronLeft, ChevronRight, Download, Edit3, Heart, Info, Maximize, Play, Trash2, X, ZoomIn, ZoomOut } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatDate } from '@/lib/format';
import { thumbhashUrl } from '@/lib/thumbhash';
import { withView } from '@/lib/routes';
import { useLocation } from '@/store/location';
import { usePrefs } from '@/store/prefs';
import { useViewer } from '@/store/viewer';
import { IconButton } from '@/components/ui';
import { useMediaActions } from '@/features/gallery/actions';
import { InfoPanel } from './InfoPanel';
import { VideoPlayer } from './VideoPlayer';

/** Zoomable, pannable image with progressive loading (thumb → 1600 → original). */
function ZoomImage({ item, hero, onSwipe, onClose }: { item: MediaItem; hero: boolean; onSwipe: (dir: 1 | -1) => void; onClose: () => void }) {
  const backend = useBackend();
  const [scale, setScale] = useState(1);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const [full, setFull] = useState(false);
  const [mid, setMid] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinch = useRef<{ d: number; scale: number } | null>(null);
  const drag = useRef<{ x: number; y: number; px: number; py: number } | null>(null);
  const zoomed = scale > 1.01;

  const clampPos = useCallback(
    (p: { x: number; y: number }, s: number) => {
      const el = box.current;
      if (!el) return p;
      const mx = (el.clientWidth * (s - 1)) / 2;
      const my = (el.clientHeight * (s - 1)) / 2;
      return { x: Math.max(-mx, Math.min(mx, p.x)), y: Math.max(-my, Math.min(my, p.y)) };
    },
    [],
  );

  const zoomAt = (next: number, cx?: number, cy?: number) => {
    const s = Math.max(1, Math.min(8, next));
    const el = box.current;
    if (el && cx !== undefined && cy !== undefined) {
      const r = el.getBoundingClientRect();
      const ox = cx - r.left - r.width / 2;
      const oy = cy - r.top - r.height / 2;
      const k = s / scale;
      setPos((p) => clampPos({ x: ox - (ox - p.x) * k, y: oy - (oy - p.y) * k }, s));
    } else if (s === 1) setPos({ x: 0, y: 0 });
    setScale(s);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === '+' || e.key === '=') zoomAt(scale * 1.4);
      if (e.key === '-') zoomAt(scale / 1.4);
      if (e.key === '0') zoomAt(1);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  const thumb = backend.thumbUrl(item.id, 640);
  const placeholder = thumbhashUrl(item.thumbhash);
  const aspect = item.width && item.height ? item.width / item.height : 1.5;

  return (
    <div
      ref={box}
      className={cn('relative flex size-full touch-none items-center justify-center overflow-hidden', zoomed ? 'cursor-grab active:cursor-grabbing' : 'cursor-zoom-in')}
      onWheel={(e) => zoomAt(scale * (e.deltaY < 0 ? 1.15 : 1 / 1.15), e.clientX, e.clientY)}
      onDoubleClick={(e) => zoomAt(zoomed ? 1 : 2.5, e.clientX, e.clientY)}
      onClick={(e) => {
        if (e.target === box.current && !zoomed) onClose();
      }}
      onPointerDown={(e) => {
        pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
        if (pointers.current.size === 2) {
          const [a, b] = [...pointers.current.values()];
          pinch.current = { d: Math.hypot(a.x - b.x, a.y - b.y), scale };
        } else if (zoomed) {
          drag.current = { x: e.clientX, y: e.clientY, px: pos.x, py: pos.y };
          (e.target as HTMLElement).setPointerCapture(e.pointerId);
        }
      }}
      onPointerMove={(e) => {
        if (!pointers.current.has(e.pointerId)) return;
        pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
        if (pinch.current && pointers.current.size === 2) {
          const [a, b] = [...pointers.current.values()];
          const d = Math.hypot(a.x - b.x, a.y - b.y);
          zoomAt((pinch.current.scale * d) / pinch.current.d, (a.x + b.x) / 2, (a.y + b.y) / 2);
        } else if (drag.current) {
          setPos(clampPos({ x: drag.current.px + e.clientX - drag.current.x, y: drag.current.py + e.clientY - drag.current.y }, scale));
        }
      }}
      onPointerUp={(e) => {
        pointers.current.delete(e.pointerId);
        if (pointers.current.size < 2) pinch.current = null;
        drag.current = null;
      }}
      onPointerCancel={(e) => {
        pointers.current.delete(e.pointerId);
        pinch.current = null;
        drag.current = null;
      }}
    >
      <motion.div
        drag={zoomed ? false : true}
        dragSnapToOrigin
        dragElastic={0.6}
        dragConstraints={{ left: 0, right: 0, top: 0, bottom: 0 }}
        onDragEnd={(_, info: PanInfo) => {
          if (Math.abs(info.offset.x) > 90 && Math.abs(info.offset.x) > Math.abs(info.offset.y)) onSwipe(info.offset.x < 0 ? 1 : -1);
          else if (info.offset.y > 140) onClose();
        }}
        className="relative flex max-h-full max-w-full items-center justify-center"
        style={{ aspectRatio: aspect, height: aspect < 1 ? '100%' : undefined, width: aspect >= 1 ? '100%' : undefined, maxHeight: '100%', maxWidth: '100%' }}
      >
        <motion.div
          className="relative size-full"
          animate={{ scale, x: pos.x, y: pos.y }}
          transition={pinch.current || drag.current ? { duration: 0 } : { type: 'spring', stiffness: 380, damping: 36 }}
        >
          <motion.img
            layoutId={hero ? `media-${item.id}` : undefined}
            src={thumb}
            alt=""
            draggable={false}
            className="absolute inset-0 size-full rounded-sm object-contain"
            style={placeholder ? { backgroundImage: `url(${placeholder})`, backgroundSize: 'contain', backgroundRepeat: 'no-repeat', backgroundPosition: 'center' } : undefined}
            transition={{ type: 'spring', stiffness: 320, damping: 34 }}
          />
          <img
            src={backend.thumbUrl(item.id, 1600)}
            alt=""
            draggable={false}
            onLoad={() => setMid(true)}
            className={cn('absolute inset-0 size-full object-contain transition-opacity duration-300', mid && !full ? 'opacity-100' : 'opacity-0')}
          />
          <img
            src={backend.displayUrl(item)}
            alt={item.name}
            draggable={false}
            onLoad={() => setFull(true)}
            className={cn('absolute inset-0 size-full object-contain transition-opacity duration-300', full ? 'opacity-100' : 'opacity-0')}
          />
        </motion.div>
      </motion.div>
    </div>
  );
}

export function Viewer() {
  const [sp] = useSearchParams();
  const id = sp.get('view');
  return <AnimatePresence>{id && <ViewerInner key="viewer" id={id} />}</AnimatePresence>;
}

function ViewerInner({ id }: { id: string }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const info = useInfo();
  const nav = useNavigate();
  const routerLoc = useRouterLocation();
  const qc = useQueryClient();
  const list = useLocation((s) => s.items);
  const { infoOpen, set: setPrefs } = usePrefs();
  const activeId = useViewer((s) => s.activeId);
  const actions = useMediaActions();
  const [dir, setDir] = useState<1 | -1>(1);
  const [chrome, setChrome] = useState(true);
  const [closing, setClosing] = useState(false);
  const chromeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const strip = useRef<HTMLDivElement>(null);

  const viewable = useMemo(() => list.filter((i) => i.kind === 'image' || i.kind === 'video'), [list]);
  const idx = viewable.findIndex((i) => i.id === id);
  const single = useQuery({
    queryKey: ['media', backend.id, id],
    queryFn: () => backend.media(id),
    enabled: idx < 0,
  });
  const item: MediaItem | undefined = idx >= 0 ? viewable[idx] : single.data;
  const prev = idx > 0 ? viewable[idx - 1] : null;
  const next = idx >= 0 && idx < viewable.length - 1 ? viewable[idx + 1] : null;

  const goTo = useCallback(
    (m: MediaItem | null, d: 1 | -1) => {
      if (!m) return;
      setDir(d);
      if (useViewer.getState().activeId) useViewer.getState().setActive(null);
      nav({ search: withView(routerLoc.search, m.id) }, { replace: true });
    },
    [nav, routerLoc.search],
  );

  const close = useCallback(() => {
    if (closing) return;
    setClosing(true);
    useViewer.getState().setActive(id);
    // give the grid tile a frame to pick up the shared layoutId, then unmount
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        nav({ search: withView(routerLoc.search, null) }, { replace: true });
        setTimeout(() => useViewer.getState().setActive(null), 700);
      }),
    );
  }, [closing, id, nav, routerLoc.search]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (/input|textarea/i.test((e.target as HTMLElement).tagName) || document.querySelector('[role=dialog]')) return;
      if (e.key === 'Escape') close();
      else if (e.key === 'ArrowRight') goTo(next, 1);
      else if (e.key === 'ArrowLeft') goTo(prev, -1);
      else if (e.key === 'i') setPrefs({ infoOpen: !usePrefs.getState().infoOpen });
      else if (e.key === 'f' && item?.kind === 'image') void document.documentElement.requestFullscreen?.();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [close, goTo, next, prev, setPrefs, item?.kind]);

  useEffect(() => {
    for (const m of [next, prev]) if (m && m.kind === 'image') new Image().src = backend.thumbUrl(m.id, 1600);
  }, [next, prev, backend]);

  useEffect(() => {
    const el = strip.current?.querySelector<HTMLElement>(`[data-strip="${id}"]`);
    el?.scrollIntoView({ inline: 'center', block: 'nearest', behavior: 'smooth' });
  }, [id]);

  const poke = () => {
    setChrome(true);
    if (chromeTimer.current) clearTimeout(chromeTimer.current);
    chromeTimer.current = setTimeout(() => setChrome(false), 3000);
  };
  useEffect(() => {
    poke();
    return () => void (chromeTimer.current && clearTimeout(chromeTimer.current));
  }, []);

  // lock page scroll behind
  useEffect(() => {
    document.body.dataset.viewer = '1';
    return () => {
      delete document.body.dataset.viewer;
    };
  }, []);

  const writable = info && !info.caps.readonly;
  const stripItems = useMemo(() => (idx >= 0 ? viewable.slice(Math.max(0, idx - 30), idx + 31) : []), [viewable, idx]);

  return (
    <motion.div className="fixed inset-0 z-[60] flex flex-col text-white" onPointerMove={poke} initial={{ opacity: 1 }} exit={{ opacity: 1 }}>
      <motion.div
        className="absolute inset-0 bg-[rgb(6_6_9)]"
        initial={{ opacity: 0 }}
        animate={{ opacity: 0.97 }}
        exit={{ opacity: 0, transition: { duration: 0.28 } }}
        transition={{ duration: 0.3 }}
      />
      {/* ambient blurred backdrop from the current image */}
      {item && item.kind === 'image' && (
        <motion.img
          key={`bg-${item.id}`}
          src={backend.thumbUrl(item.id, 320)}
          alt=""
          aria-hidden
          className="pointer-events-none absolute inset-0 size-full scale-110 object-cover opacity-30 blur-[80px] saturate-150"
          initial={{ opacity: 0 }}
          animate={{ opacity: 0.32 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.6 }}
        />
      )}

      {/* top bar */}
      <motion.div
        className="relative z-10 flex h-16 shrink-0 items-center gap-2 px-3 sm:px-5"
        initial={{ opacity: 0, y: -20 }}
        animate={{ opacity: chrome ? 1 : 0, y: chrome ? 0 : -20 }}
        exit={{ opacity: 0, y: -20 }}
      >
        <IconButton label={t('common.close')} tone="glass" onClick={close}>
          <X />
        </IconButton>
        {item && (
          <div className="ml-2 min-w-0 flex-1">
            <div className="truncate text-sm font-semibold">{item.name}</div>
            <div className="truncate text-xs text-white/60">
              {formatDate(item.takenAt, lang, { dateStyle: 'long', timeStyle: 'short' })}
              {idx >= 0 && ` · ${idx + 1} / ${viewable.length}`}
            </div>
          </div>
        )}
        {item && (
          <div className="flex items-center gap-1.5">
            {item.kind === 'image' && (
              <>
                <IconButton label={t('viewer.zoomOut')} tone="glass" className="hidden sm:inline-flex" onClick={() => window.dispatchEvent(new KeyboardEvent('keydown', { key: '-' }))}>
                  <ZoomOut />
                </IconButton>
                <IconButton label={t('viewer.zoomIn')} tone="glass" className="hidden sm:inline-flex" onClick={() => window.dispatchEvent(new KeyboardEvent('keydown', { key: '+' }))}>
                  <ZoomIn />
                </IconButton>
              </>
            )}
            <IconButton
              label={t('viewer.favorite')}
              tone="glass"
              onClick={async () => {
                await actions.favorite([item], !item.favorite);
                void qc.invalidateQueries({ queryKey: ['media', backend.id, item.id] });
              }}
            >
              <Heart className={cn(item.favorite && 'fill-[var(--accent-2)] text-[var(--accent-2)]')} />
            </IconButton>
            {writable && (
              <IconButton label={t('viewer.edit')} tone="glass" onClick={() => actions.edit(item)}>
                <Edit3 />
              </IconButton>
            )}
            <IconButton label={t('viewer.slideshow')} tone="glass" className="hidden sm:inline-flex" onClick={() => actions.slideshow(idx >= 0 ? [...viewable.slice(idx), ...viewable.slice(0, idx)] : [item])}>
              <Play />
            </IconButton>
            <IconButton label={t('viewer.download')} tone="glass" onClick={() => actions.download([item])}>
              <Download />
            </IconButton>
            {writable && (
              <IconButton
                label={t('viewer.delete')}
                tone="glass"
                onClick={() => {
                  actions.remove([item]);
                }}
              >
                <Trash2 />
              </IconButton>
            )}
            <IconButton label={t('viewer.info')} tone="glass" active={infoOpen} onClick={() => setPrefs({ infoOpen: !infoOpen })}>
              <Info />
            </IconButton>
            <IconButton label={t('viewer.fullscreen')} tone="glass" className="hidden sm:inline-flex" onClick={() => (document.fullscreenElement ? document.exitFullscreen() : document.documentElement.requestFullscreen())}>
              <Maximize />
            </IconButton>
          </div>
        )}
      </motion.div>

      {/* stage */}
      <div className={cn('relative z-0 min-h-0 flex-1 transition-[padding] duration-300', infoOpen && 'sm:pr-[340px]')}>
        <AnimatePresence initial={false} custom={dir} mode="popLayout">
          {item && (
            <motion.div
              key={item.id}
              custom={dir}
              className="absolute inset-0 px-2 py-2 sm:px-16"
              initial={activeId === item.id ? false : { opacity: 0, x: dir * 120 }}
              animate={{ opacity: 1, x: 0 }}
              exit={closing ? { opacity: 1 } : { opacity: 0, x: dir * -120 }}
              transition={{ type: 'spring', stiffness: 320, damping: 34 }}
            >
              {item.kind === 'video' ? (
                <div className="flex size-full items-center justify-center">
                  <VideoPlayer item={item} />
                </div>
              ) : (
                <ZoomImage item={item} hero={activeId === item.id} onSwipe={(d) => goTo(d === 1 ? next : prev, d)} onClose={close} />
              )}
            </motion.div>
          )}
        </AnimatePresence>

        {prev && (
          <motion.button
            type="button"
            aria-label={t('viewer.prev')}
            onClick={() => goTo(prev, -1)}
            animate={{ opacity: chrome ? 1 : 0 }}
            className="glass absolute top-1/2 left-3 z-10 hidden size-12 -translate-y-1/2 place-items-center rounded-full border border-white/10 text-white hover:bg-white/15 sm:grid"
          >
            <ChevronLeft className="size-6" />
          </motion.button>
        )}
        {next && (
          <motion.button
            type="button"
            aria-label={t('viewer.next')}
            onClick={() => goTo(next, 1)}
            animate={{ opacity: chrome ? 1 : 0 }}
            className={cn('glass absolute top-1/2 right-3 z-10 hidden size-12 -translate-y-1/2 place-items-center rounded-full border border-white/10 text-white hover:bg-white/15 sm:grid', infoOpen && 'sm:right-[352px]')}
          >
            <ChevronRight className="size-6" />
          </motion.button>
        )}
        <AnimatePresence>{infoOpen && item && <InfoPanel item={item} onClose={() => setPrefs({ infoOpen: false })} />}</AnimatePresence>
      </div>

      {/* filmstrip */}
      {stripItems.length > 1 && (
        <motion.div
          ref={strip}
          className={cn('no-scrollbar relative z-10 flex h-20 shrink-0 items-center gap-1.5 overflow-x-auto px-[50%] pb-3', infoOpen && 'sm:mr-[340px]')}
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: chrome ? 1 : 0, y: chrome ? 0 : 20 }}
          exit={{ opacity: 0, y: 20 }}
        >
          {stripItems.map((m) => (
            <button
              key={m.id}
              type="button"
              data-strip={m.id}
              onClick={() => goTo(m, viewable.indexOf(m) > idx ? 1 : -1)}
              className={cn('relative h-14 shrink-0 overflow-hidden rounded-lg transition-all duration-300', m.id === id ? 'w-20 opacity-100 ring-2 ring-white' : 'w-10 opacity-50 hover:opacity-90')}
            >
              <img src={backend.thumbUrl(m.id, 320)} alt="" className="size-full object-cover" draggable={false} />
            </button>
          ))}
        </motion.div>
      )}
    </motion.div>
  );
}
