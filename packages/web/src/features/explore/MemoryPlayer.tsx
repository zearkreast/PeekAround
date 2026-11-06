import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useNavigate } from 'react-router';
import { useQuery } from '@tanstack/react-query';
import { motion, animate as animateValue, useMotionValue, useTransform } from 'motion/react';
import { Grid3x3, Pause, Play, Presentation, Volume2, VolumeX, X } from 'lucide-react';
import type { MediaItem, Memory } from '@peekaround/shared';
import { spreadPick } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { BACK, canGoBackInHistory, useBackHandler } from '@/lib/backStack';
import { Spinner } from '@/components/ui';
import { usePlaySlideshow } from './data';
import { exploreQuery, memoryHref, memoryLabel } from './labels';
import { useResolved } from './ResultPage';

const PHOTO_MS = 5000;
const TITLE_MS = 3200;
const VIDEO_MAX_MS = 15000;
const MAX_SLIDES = 60;
const HOLD_MS = 220;

type Slide = { kind: 'title' } | { kind: 'item'; item: MediaItem };

/** items of the memory in chronological order, at most MAX_SLIDES spread evenly over it */
function useMemoryItems(m: Memory | undefined) {
  const backend = useBackend();
  return useQuery({
    queryKey: ['explore', backend.id, 'memory-items', m?.id, m?.from, m?.to],
    enabled: !!m,
    staleTime: 60_000,
    queryFn: async () => {
      // year in review: a few items from every month instead of the first 2000 of the year
      if (m!.kind === 'year') {
        const year = new Date(m!.from).getUTCFullYear();
        const per = Math.ceil(MAX_SLIDES / Math.max(1, m!.months ? m!.months.filter((n) => n > 0).length : 12));
        const out: MediaItem[] = [];
        for (let mo = 0; mo < 12; mo++) {
          if (m!.months && !m!.months[mo]) continue;
          const q = { ...exploreQuery({ from: Date.UTC(year, mo, 1), to: Date.UTC(year, mo + 1, 1) }), order: 'asc' as const };
          const r = await backend.browse({ ...q, offset: 0, limit: 400 });
          const list = r.items.filter((i) => i.kind === 'image' || i.kind === 'video');
          const keep = new Set(spreadPick(list, per));
          out.push(...list.filter((i) => keep.has(i.id)));
        }
        return out.slice(0, MAX_SLIDES + 12);
      }
      const q = { ...exploreQuery({ from: m!.from, to: m!.to }), order: 'asc' as const };
      const r = await backend.browse({ ...q, offset: 0, limit: 2000 });
      const list = r.items.filter((i) => i.kind === 'image' || i.kind === 'video');
      if (list.length <= MAX_SLIDES) return list;
      const keep = new Set(spreadPick(list, MAX_SLIDES));
      return list.filter((i) => keep.has(i.id));
    },
  });
}

export function MemoryPlayerRoute() {
  const nav = useNavigate();
  const res = useResolved('memory');
  const items = useMemoryItems(res.memory);
  const close = useCallback(() => {
    if (canGoBackInHistory()) nav(-1);
    else nav(res.memory ? memoryHref(res.memory.id) : '/explore', { replace: true });
  }, [nav, res.memory]);
  return createPortal(
    <MemoryPlayer memory={res.memory} items={items.data} missing={res.missing} onClose={close} />,
    document.body,
  );
}

export function MemoryPlayer({ memory, items, missing, onClose }: { memory: Memory | undefined; items: MediaItem[] | undefined; missing?: boolean; onClose: () => void }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const nav = useNavigate();
  const playSlides = usePlaySlideshow();
  const label = memory ? memoryLabel(memory, lang) : null;
  const slides: Slide[] = useMemo(() => (items ? [{ kind: 'title' }, ...items.map((item) => ({ kind: 'item' as const, item }))] : []), [items]);
  const [index, setIndex] = useState(0);
  const [paused, setPaused] = useState(false);
  const [muted, setMuted] = useState(true);
  const [ui, setUi] = useState(true);
  const bars = useRef<(HTMLElement | null)[]>([]);
  const elapsed = useRef(0);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const holdTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const held = useRef(false);
  const cur = slides[index];
  const dragY = useMotionValue(0);
  const bgOpacity = useTransform(dragY, [0, 400], [1, 0.2]);
  const scale = useTransform(dragY, [0, 400], [1, 0.86]);

  useBackHandler(true, onClose, BACK.viewer, 'memory-player');

  // lock page scroll underneath
  useEffect(() => {
    const prev = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => void (document.body.style.overflow = prev);
  }, []);

  const indexRef = useRef(0);
  indexRef.current = index;
  const go = useCallback(
    (d: 1 | -1) => {
      const n = indexRef.current + d;
      if (n < 0) return;
      if (n >= slides.length) {
        onClose();
        return;
      }
      indexRef.current = n;
      setIndex(n);
    },
    [slides.length, onClose],
  );

  // reset progress on slide change; bars before = full, after = empty
  useEffect(() => {
    elapsed.current = 0;
    bars.current.forEach((b, k) => {
      if (!b) return;
      b.dataset.state = k < index ? 'done' : k === index ? 'active' : 'todo';
      const fill = b.firstElementChild as HTMLElement | null;
      if (fill) fill.style.transform = `scaleX(${k < index ? 1 : 0})`;
    });
  }, [index, slides.length]);

  useEffect(() => {
    for (const k of [1, 2]) {
      const s = slides[index + k];
      if (s?.kind === 'item' && s.item.kind === 'image') {
        const img = new Image();
        img.decoding = 'async';
        img.src = backend.thumbUrl(s.item.id, 1600);
      }
    }
  }, [index, slides, backend]);

  // progress clock (rAF, writes the active bar directly — no re-render per frame)
  useEffect(() => {
    if (!cur) return;
    let raf = 0;
    let last = performance.now();
    const tick = (now: number) => {
      const dt = now - last;
      last = now;
      let p = 0;
      if (cur.kind === 'item' && cur.item.kind === 'video') {
        const v = videoRef.current;
        if (v && Number.isFinite(v.duration) && v.duration > 0) {
          const dur = Math.min(v.duration * 1000, VIDEO_MAX_MS);
          p = Math.min(1, (v.currentTime * 1000) / dur);
          if (v.ended || v.currentTime * 1000 >= dur) p = 1;
        } else if (!paused) {
          // not playable / still loading: fall back to a photo-like clock
          elapsed.current += dt;
          p = Math.min(1, elapsed.current / (VIDEO_MAX_MS / 2));
        }
      } else {
        if (!paused) elapsed.current += dt;
        p = Math.min(1, elapsed.current / (cur.kind === 'title' ? TITLE_MS : PHOTO_MS));
      }
      const fill = bars.current[index]?.firstElementChild as HTMLElement | null;
      if (fill) fill.style.transform = `scaleX(${p})`;
      if (p >= 1) {
        go(1);
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [cur, index, paused, go]);

  // video follows pause state
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    if (paused) v.pause();
    else void v.play().catch(() => {});
  }, [paused, index]);

  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if (e.key === 'ArrowRight') go(1);
      else if (e.key === 'ArrowLeft') go(-1);
      else if (e.key === ' ') {
        e.preventDefault();
        setPaused((p) => !p);
      }
    };
    window.addEventListener('keydown', on);
    return () => window.removeEventListener('keydown', on);
  }, [go]);

  // ── gestures: tap left/right, hold = pause, swipe down = close ──
  const start = useRef<{ x: number; y: number; t: number; id: number } | null>(null);
  const onPointerDown = (e: React.PointerEvent) => {
    if ((e.target as HTMLElement).closest('[data-mp-control]')) return;
    start.current = { x: e.clientX, y: e.clientY, t: performance.now(), id: e.pointerId };
    held.current = false;
    holdTimer.current = setTimeout(() => {
      held.current = true;
      setPaused(true);
      setUi(false);
    }, HOLD_MS);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const s = start.current;
    if (!s || s.id !== e.pointerId) return;
    const dy = e.clientY - s.y;
    if (Math.abs(dy) > 8 || Math.abs(e.clientX - s.x) > 8) {
      if (holdTimer.current) clearTimeout(holdTimer.current);
      if (!held.current) {
        held.current = true;
        setPaused(true);
      }
    }
    if (dy > 0) dragY.set(dy);
  };
  const endPointer = (e: React.PointerEvent) => {
    const s = start.current;
    start.current = null;
    if (holdTimer.current) clearTimeout(holdTimer.current);
    if (!s || s.id !== e.pointerId) return;
    const dy = e.clientY - s.y;
    const dx = e.clientX - s.x;
    if (dy > 110 || (dy > 50 && dy / Math.max(1, performance.now() - s.t) > 0.6)) {
      onClose();
      return;
    }
    void animateValue(dragY, 0, { type: 'spring', stiffness: 420, damping: 38 });
    if (held.current) {
      held.current = false;
      setPaused(false);
      setUi(true);
      // horizontal swipe = navigate
      if (Math.abs(dx) > 60 && Math.abs(dx) > Math.abs(dy)) go(dx < 0 ? 1 : -1);
      return;
    }
    const w = window.innerWidth;
    go(e.clientX < w * 0.3 ? -1 : 1);
  };

  const openGrid = () => memory && nav(memoryHref(memory.id), { replace: true });
  const asSlideshow = () => items && playSlides(items);

  const kb = index % 2 ? 'kenburns-b' : 'kenburns-a';
  const cover = memory?.coverIds[0];

  return (
    <motion.div
      className={cn('fixed inset-0 z-[95] touch-none overflow-hidden text-white select-none', paused && 'mp-paused')}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      role="dialog"
      aria-modal="true"
      aria-label={t('explore.player.label')}
      data-memory-player
      data-index={index}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endPointer}
      onPointerCancel={endPointer}
    >
      <motion.div className="absolute inset-0 bg-black" style={{ opacity: bgOpacity }} />
      <motion.div className="absolute inset-0" style={{ y: dragY, scale }}>
        {!cur && (
          <div className="grid size-full place-items-center">{missing ? <p className="px-8 text-center text-white/80">{t('explore.notFound')}</p> : <Spinner className="size-8" />}</div>
        )}
        {cur?.kind === 'title' && (
          <div key="title" className="absolute inset-0">
            {cover && <img src={backend.thumbUrl(cover, 1600)} alt="" className={cn('absolute inset-0 size-full object-cover opacity-55', kb)} style={{ ['--kb-dur' as string]: `${TITLE_MS + 1500}ms` }} />}
            <div className="absolute inset-0 bg-gradient-to-b from-black/30 via-black/20 to-black/70" />
            <motion.div initial={{ opacity: 0, y: 18 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.6, ease: [0.22, 1, 0.36, 1] }} className="absolute inset-x-0 bottom-[22%] px-8 text-center">
              <h2 className="font-display text-[34px] leading-tight font-bold [text-wrap:balance] sm:text-5xl">{label?.title}</h2>
              <p className="mt-3 text-[15px] text-white/80">{label?.subtitle}</p>
            </motion.div>
          </div>
        )}
        {cur?.kind === 'item' && (
          <div key={cur.item.id} className="absolute inset-0 grid place-items-center">
            {cur.item.kind === 'video' ? (
              <video
                ref={videoRef}
                src={cur.item.playback === 'transcode' ? backend.streamUrl(cur.item.id) : backend.fileUrl(cur.item.id)}
                poster={backend.thumbUrl(cur.item.id, 1600)}
                muted={muted}
                playsInline
                autoPlay
                preload="auto"
                className="max-h-full max-w-full object-contain"
              />
            ) : (
              <>
                {/* backdrop: same image, dimmed, covering (no blur filter: cheap on phones) */}
                <img src={backend.thumbUrl(cur.item.id, 320)} alt="" aria-hidden className="absolute inset-0 size-full scale-110 object-cover opacity-35" />
                <img
                  src={backend.thumbUrl(cur.item.id, 1600)}
                  alt={cur.item.name}
                  decoding="async"
                  className={cn('relative max-h-full max-w-full object-contain', kb)}
                  style={{ ['--kb-dur' as string]: `${PHOTO_MS + 1200}ms` }}
                  draggable={false}
                />
              </>
            )}
          </div>
        )}
      </motion.div>

      {/* top: progress bars + controls */}
      <div className={cn('wco-r absolute inset-x-0 top-0 z-10 bg-gradient-to-b from-black/55 to-transparent pb-10 pl-3 [--wco-base:0.75rem] transition-opacity duration-200', !ui && 'opacity-0')} style={{ paddingTop: 'calc(env(safe-area-inset-top, 0px) + 10px)' }}>
        <div className="flex gap-1">
          {slides.map((_, k) => (
            <span key={k} className="mp-bar" ref={(n) => void (bars.current[k] = n)}>
              <i />
            </span>
          ))}
        </div>
        <div className="mt-3 flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-semibold">{label?.title}</div>
            <div className="truncate text-xs text-white/70">{label?.subtitle}</div>
          </div>
          {cur?.kind === 'item' && cur.item.kind === 'video' && (
            <button type="button" data-mp-control className="grid size-10 place-items-center rounded-full hover:bg-white/15" aria-label={muted ? t('explore.player.unmute') : t('explore.player.mute')} onClick={() => setMuted((m) => !m)}>
              {muted ? <VolumeX className="size-5" /> : <Volume2 className="size-5" />}
            </button>
          )}
          <button type="button" data-mp-control data-action="pause" className="grid size-10 place-items-center rounded-full hover:bg-white/15" aria-label={paused ? t('explore.player.resume') : t('explore.player.pause')} onClick={() => setPaused((p) => !p)}>
            {paused ? <Play className="size-5" /> : <Pause className="size-5" />}
          </button>
          <button type="button" data-mp-control data-action="close" className="grid size-10 place-items-center rounded-full hover:bg-white/15" aria-label={t('common.close')} onClick={onClose}>
            <X className="size-6" />
          </button>
        </div>
      </div>

      {/* bottom actions */}
      <div className={cn('absolute inset-x-0 bottom-0 z-10 flex items-center justify-center gap-2 bg-gradient-to-t from-black/60 to-transparent px-4 pt-12 transition-opacity duration-200', !ui && 'opacity-0')} style={{ paddingBottom: 'calc(env(safe-area-inset-bottom, 0px) + 18px)' }}>
        <button type="button" data-mp-control onClick={openGrid} className="inline-flex h-10 items-center gap-2 rounded-full bg-white/15 px-4 text-sm font-medium hover:bg-white/25">
          <Grid3x3 className="size-4" />
          {t('explore.player.openGrid')}
        </button>
        <button type="button" data-mp-control onClick={asSlideshow} className="inline-flex h-10 items-center gap-2 rounded-full bg-white px-4 text-sm font-semibold text-black hover:bg-white/90" disabled={!items?.length}>
          <Presentation className="size-4" />
          {t('explore.player.asSlideshow')}
        </button>
      </div>
      {/* edge hit areas for keyboard / screen readers */}
      <button type="button" data-mp-control className="sr-only" onClick={() => go(-1)}>
        {t('explore.player.prev')}
      </button>
      <button type="button" data-mp-control className="sr-only" onClick={() => go(1)}>
        {t('explore.player.next')}
      </button>
    </motion.div>
  );
}
