import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useNavigate, useSearchParams, useLocation as useRouterLocation } from 'react-router';
import { AnimatePresence, animate, motion, useMotionValue, useTransform } from 'motion/react';
import { useGesture } from '@use-gesture/react';
import { useQuery } from '@tanstack/react-query';
import { ArrowLeft, ChevronLeft, ChevronRight, Info, Maximize, MoreVertical, Music, Presentation, X, ZoomIn, ZoomOut } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { ApiError } from '@/backend/types';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatDate, formatLocal } from '@/lib/format';
import { hasPicture, mobileBarOrder, viewable } from '@/lib/mediaKinds';
import { thumbhashUrl } from '@/lib/thumbhash';
import { withView } from '@/lib/routes';
import { BACK, useBackHandler } from '@/lib/backStack';
import { useIsMobile } from '@/lib/useMedia';
import { useLocation } from '@/store/location';
import { usePrefs } from '@/store/prefs';
import { useViewer } from '@/store/viewer';
import { BottomSheet, IconButton, MenuItem as MenuRow, Popover as DesktopMenu, SheetItem } from '@/components/ui';
import { iconOf, labelOf, shortLabelOf, useMediaActions, type MediaAction } from '@/features/gallery/actions';
import { useSourceVersion, type GridItems } from '@/features/gallery/grid/source';
import { InfoContent, InfoPanel } from './InfoPanel';
import { Slide, aspectOfItem, fitContain, type ZoomValues } from './Slide';
import { decodeImage } from './imageCache';

const GAP = 24;
const MAX_ZOOM = 8;
const HIRES_AT = 1.5;
const spring = { type: 'spring', stiffness: 340, damping: 36 } as const;


/** nearest viewable item from index `i` in direction `d`; null when not loaded yet */
function step(src: GridItems, i: number, d: 1 | -1): { item: MediaItem; index: number } | null {
  for (let k = i + d, n = 0; k >= 0 && k < src.total && n < 50; k += d, n++) {
    const it = src.getAt(k);
    if (!it) return null;
    if (viewable(it)) return { item: it, index: k };
  }
  return null;
}

const rubber = (v: number, min: number, max: number) => (v < min ? min + (v - min) * 0.3 : v > max ? max + (v - max) * 0.3 : v);
const clamp = (v: number, min: number, max: number) => Math.max(min, Math.min(max, v));

export function Viewer() {
  const [sp] = useSearchParams();
  const id = sp.get('view');
  return <AnimatePresence>{id && <ViewerInner key="viewer" id={id} />}</AnimatePresence>;
}

function ViewerInner({ id }: { id: string }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const nav = useNavigate();
  const routerLoc = useRouterLocation();
  const source = useLocation((s) => s.source);
  useSourceVersion(source);
  const infoOpen = usePrefs((s) => s.infoOpen);
  const setPrefs = usePrefs((s) => s.set);
  // Settings → viewer
  const viewerBg = usePrefs((s) => s.viewerBg);
  const filmstrip = usePrefs((s) => s.filmstrip);
  const zoomStep = usePrefs((s) => s.zoomStep);
  const dblZoom = usePrefs((s) => s.dblZoom);
  const hideLocation = usePrefs((s) => s.hideLocation);
  useEffect(() => {
    const d = usePrefs.getState().infoDefault;
    if (d !== 'remember') usePrefs.getState().set({ infoOpen: d === 'open' });
  }, []);
  const activeId = useViewer((s) => s.activeId);
  const mobile = useIsMobile();
  const actions = useMediaActions('viewer');
  const [chrome, setChrome] = useState(true);
  const [closing, setClosing] = useState(false);
  const [infoSheet, setInfoSheet] = useState(false);
  const [menuSheet, setMenuSheet] = useState(false);
  const [hiRes, setHiRes] = useState(false);
  const [zoomed, setZoomed] = useState(false);
  const [swiping, setSwiping] = useState(false);
  const chromeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const strip = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ W: 0, H: 0 });

  // ── where are we in the list ──
  const index = source ? source.indexOf(id) : -1;
  const single = useQuery({
    queryKey: ['media', backend.id, id],
    queryFn: () => backend.media(id),
    enabled: index < 0,
    retry: (n, e) => !(e instanceof ApiError && e.status === 404) && n < 1,
  });
  const item: MediaItem | undefined = index >= 0 ? source!.getAt(index) : single.data;
  const prev = source && index >= 0 ? step(source, index, -1) : null;
  const next = source && index >= 0 ? step(source, index, 1) : null;

  // neighbours of the current item, so the viewer can move on when it disappears (deleted / moved)
  const neighbours = useRef<{ id: string; list: MediaItem[] } | null>(null);
  if (index >= 0) neighbours.current = { id, list: [next?.item, prev?.item].filter((m): m is MediaItem => !!m) };

  // keep the pages around the current item loaded (also beyond the grid's loaded range)
  useEffect(() => {
    if (source && index >= 0) source.ensureRange(Math.max(0, index - 6), index + 6, 1);
  }, [source, index]);

  // preload ±2 neighbours at 1600 (decoded); far slides are released by the LRU
  useEffect(() => {
    if (!source || index < 0) return;
    const t0 = setTimeout(() => {
      for (const k of [1, -1, 2, -2]) {
        const it = source.getAt(index + k);
        if (it && hasPicture(it)) void decodeImage(backend.thumbUrl(it.id, 1600));
      }
    }, 120);
    return () => clearTimeout(t0);
  }, [source, index, backend]);

  // ── motion values (rendering only; gestures write to them directly) ──
  const trackX = useMotionValue(0);
  const dismissY = useMotionValue(0);
  const liftY = useMotionValue(0);
  const scale = useMotionValue(1);
  /** scale the current zoom animation is heading to */
  const zoomGoal = useRef(1);
  const zx = useMotionValue(0);
  const zy = useMotionValue(0);
  const zoomVals: ZoomValues = useMemo(() => ({ scale, x: zx, y: zy }), [scale, zx, zy]);
  const one = useMotionValue(1);
  const zero = useMotionValue(0);
  const staticZoom: ZoomValues = useMemo(() => ({ scale: one, x: zero, y: zero }), [one, zero]);
  const slideY = useTransform([dismissY, liftY], ([d, l]: number[]) => d + l);
  const slideScale = useTransform(dismissY, (d) => 1 - Math.min(0.3, d / Math.max(1, size.H) / 1.4));
  const bgOpacity = useTransform(dismissY, (d) => 1 - Math.min(1, d / Math.max(1, size.H * 0.55)));

  useLayoutEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const measure = () => setSize({ W: el.clientWidth, H: el.clientHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // new current item → reset pager / zoom in the same frame the slides re-key
  useLayoutEffect(() => {
    for (const mv of [trackX, dismissY, liftY, zx, zy]) {
      mv.stop();
      mv.set(0);
    }
    scale.stop();
    scale.set(1);
    setHiRes(false);
    setZoomed(false);
  }, [id, trackX, dismissY, liftY, zx, zy, scale]);

  // ── navigation ──
  const goTo = useCallback(
    (m: MediaItem | null | undefined) => {
      if (!m) return;
      if (useViewer.getState().activeId) useViewer.getState().setActive(null);
      nav({ search: withView(routerLoc.search, m.id) }, { replace: true, state: (history.state as { usr?: unknown } | null)?.usr ?? null });
    },
    [nav, routerLoc.search],
  );

  const resetZoom = useCallback(() => {
    zoomGoal.current = 1;
    animate(scale, 1, spring);
    animate(zx, 0, spring);
    animate(zy, 0, spring);
    setZoomed(false);
  }, [scale, zx, zy]);

  const close = useCallback(() => {
    if (closing) return;
    setClosing(true);
    scale.set(1);
    zx.set(0);
    zy.set(0);
    useViewer.getState().setActive(id);
    const pushed = !!(history.state as { usr?: { viewer?: boolean } } | null)?.usr?.viewer;
    // give the grid tile a frame to pick up the shared layoutId, then unmount
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        if (pushed) nav(-1);
        else nav({ search: withView(routerLoc.search, null) }, { replace: true });
        setTimeout(() => useViewer.getState().setActive(null), 700);
      }),
    );
  }, [closing, id, nav, routerLoc.search, scale, zx, zy]);

  useBackHandler(!closing, close, BACK.viewer, 'viewer');

  // the current item is gone (deleted / moved away, or a stale ?view= link): show a neighbour or close
  const gone = index < 0 && single.isError;
  useEffect(() => {
    if (!gone || closing) return;
    const nb = neighbours.current?.id === id ? neighbours.current.list[0] : undefined;
    if (nb) goTo(nb);
    else close();
  }, [gone, id, closing, goTo, close]);

  /** slide the pager is animating to (keys / buttons pressed again meanwhile commit it at once) */
  const paging = useRef<{ item: MediaItem; index: number } | null>(null);
  useLayoutEffect(() => {
    paging.current = null;
  }, [id]);

  /** animate the pager one slide and commit */
  const page = useCallback(
    (dir: 1 | -1, velocity = 0) => {
      const busy = paging.current;
      if (busy) {
        // still sliding: commit the pending slide and step on from there without animating,
        // so quick repeated presses are never lost
        paging.current = null;
        trackX.stop();
        goTo((source && step(source, busy.index, dir)?.item) || busy.item);
        return;
      }
      const target = dir === 1 ? next : prev;
      if (!target || !size.W) {
        animate(trackX, 0, spring);
        return;
      }
      paging.current = target;
      animate(trackX, -dir * (size.W + GAP), {
        type: 'spring',
        stiffness: 300,
        damping: 34,
        velocity,
        onComplete: () => {
          if (paging.current !== target) return;
          paging.current = null;
          goTo(target.item);
        },
      });
    },
    [next, prev, size.W, trackX, goTo, source],
  );

  // ── chrome visibility ──
  const poke = useCallback(() => {
    if (mobile) return;
    setChrome(true);
    if (chromeTimer.current) clearTimeout(chromeTimer.current);
    chromeTimer.current = setTimeout(() => setChrome(false), 3000);
  }, [mobile]);
  useEffect(() => {
    poke();
    return () => void (chromeTimer.current && clearTimeout(chromeTimer.current));
  }, [poke]);

  // ── zoom helpers ──
  const bounds = useCallback(
    (s: number) => {
      if (!item) return { x: 0, y: 0 };
      const fit = fitContain(size.W, size.H, aspectOfItem(item));
      return { x: Math.max(0, (fit.w * s - size.W) / 2), y: Math.max(0, (fit.h * s - size.H) / 2) };
    },
    [item, size],
  );
  const zoomTo = useCallback(
    (s: number, cx = 0, cy = 0, animated = true) => {
      const s0 = scale.get();
      const ns = clamp(s, 1, MAX_ZOOM);
      const k = ns / s0;
      const b = bounds(ns);
      const nx = clamp(cx - (cx - zx.get()) * k, -b.x, b.x);
      const ny = clamp(cy - (cy - zy.get()) * k, -b.y, b.y);
      if (animated) {
        animate(scale, ns, spring);
        animate(zx, nx, spring);
        animate(zy, ny, spring);
      } else {
        scale.set(ns);
        zx.set(nx);
        zy.set(ny);
      }
      zoomGoal.current = ns;
      setZoomed(ns > 1.02);
      if (ns > HIRES_AT) setHiRes(true);
    },
    [bounds, scale, zx, zy],
  );
  /** zoom relative to the current target (repeated +/- presses compound while animating) */
  const zoomBy = useCallback((k: number) => zoomTo((scale.isAnimating() ? zoomGoal.current : scale.get()) * k), [zoomTo, scale]);
  const settleZoom = useCallback(() => {
    const s = scale.get();
    if (s <= 1.02) {
      resetZoom();
      return;
    }
    const ns = Math.min(MAX_ZOOM, s);
    const b = bounds(ns);
    zoomGoal.current = ns;
    animate(scale, ns, spring);
    animate(zx, clamp(zx.get(), -b.x, b.x), spring);
    animate(zy, clamp(zy.get(), -b.y, b.y), spring);
    setZoomed(true);
    if (ns > HIRES_AT) setHiRes(true);
  }, [scale, zx, zy, bounds, resetZoom]);

  const center = (x: number, y: number) => {
    const r = stageRef.current!.getBoundingClientRect();
    return [x - (r.left + r.width / 2), y - (r.top + r.height / 2)] as const;
  };

  // ── gestures: @use-gesture is the single owner ──
  const g = useRef({ axis: null as null | 'x' | 'y', pinching: false, lastTap: 0, tapX: 0, tapY: 0, tapTimer: 0 as unknown as ReturnType<typeof setTimeout>, panX: 0, panY: 0 });
  const onTap = (x: number, y: number, pointerType: string) => {
    const s = g.current;
    const now = performance.now();
    if (now - s.lastTap < 280 && Math.hypot(x - s.tapX, y - s.tapY) < 40) {
      clearTimeout(s.tapTimer);
      s.lastTap = 0;
      if (item?.kind !== 'image') return;
      if (scale.get() > 1.02) resetZoom();
      else {
        const [cx, cy] = center(x, y);
        zoomTo(dblZoom, cx, cy);
        if (mobile) setChrome(false);
      }
      return;
    }
    s.lastTap = now;
    s.tapX = x;
    s.tapY = y;
    clearTimeout(s.tapTimer);
    s.tapTimer = setTimeout(() => {
      if (pointerType === 'mouse') {
        // click outside the picture closes (desktop)
        const el = stageRef.current?.querySelector('[data-viewer-active] [data-zoom-target]');
        const r = el?.getBoundingClientRect();
        if (r && (x < r.left || x > r.right || y < r.top || y > r.bottom) && scale.get() <= 1.02) close();
      } else setChrome((c) => !c);
    }, 260);
  };

  useGesture(
    {
      onDrag: ({ first, last, movement: [mx, my], velocity: [vx, vy], direction: [dx, dy], tap, xy: [x, y], event, cancel, pinching }) => {
        const s = g.current;
        if ((event.target as HTMLElement | null)?.closest?.('[data-no-gesture]')) return;
        if (pinching || s.pinching) return cancel();
        if (tap) {
          // use-gesture reports a filtered tap once, on release
          onTap(x, y, (event as PointerEvent).pointerType ?? 'mouse');
          return;
        }
        if (first) {
          s.axis = null;
          s.panX = zx.get();
          s.panY = zy.get();
          trackX.stop();
          dismissY.stop();
          // grabbed while the pager was still sliding → land on that slide first
          const busy = paging.current;
          if (busy) {
            paging.current = null;
            goTo(busy.item);
          }
        }
        // zoomed → pan with rubber band, then settle with inertia
        if (scale.get() > 1.02) {
          const b = bounds(scale.get());
          zx.set(rubber(s.panX + mx, -b.x, b.x));
          zy.set(rubber(s.panY + my, -b.y, b.y));
          if (last) {
            animate(zx, clamp(zx.get() + dx * vx * 160, -b.x, b.x), spring);
            animate(zy, clamp(zy.get() + dy * vy * 160, -b.y, b.y), spring);
          }
          return;
        }
        if (!s.axis) {
          if (Math.abs(mx) < 8 && Math.abs(my) < 8) return;
          s.axis = Math.abs(mx) >= Math.abs(my) ? 'x' : 'y';
          if (s.axis === 'x') {
            setSwiping(true);
            stageRef.current?.querySelector('video')?.pause();
          }
        }
        if (s.axis === 'x') {
          const edge = (!prev && mx > 0) || (!next && mx < 0);
          trackX.set(edge ? mx * 0.25 : mx);
          if (last) {
            setSwiping(false);
            const W = size.W || 1;
            const fling = vx > 0.3 && Math.abs(mx) > 24;
            if (mx < 0 && next && (mx < -W * 0.28 || (fling && dx < 0))) page(1, -vx * 1000);
            else if (mx > 0 && prev && (mx > W * 0.28 || (fling && dx > 0))) page(-1, vx * 1000);
            else animate(trackX, 0, spring);
          }
          return;
        }
        // vertical: down = dismiss, up = details
        if (my >= 0) {
          dismissY.set(my);
          liftY.set(0);
        } else {
          dismissY.set(0);
          liftY.set(my * 0.3);
        }
        if (last) {
          if (my > 110 || (vy > 0.45 && dy > 0 && my > 30)) close();
          else animate(dismissY, 0, spring);
          if (my < -70 || (vy > 0.4 && dy < 0 && my < -25)) {
            if (mobile) setInfoSheet(true);
            else setPrefs({ infoOpen: true });
          }
          animate(liftY, 0, spring);
        }
      },
      onPinch: ({ first, last, offset: [s], origin: [ox, oy], memo, cancel }) => {
        if (item?.kind !== 'image') return cancel();
        const st = g.current;
        let m = memo as { s0: number; x0: number; y0: number; cx: number; cy: number; ox: number; oy: number } | undefined;
        if (first || !m) {
          st.pinching = true;
          const [cx, cy] = center(ox, oy);
          m = { s0: scale.get(), x0: zx.get(), y0: zy.get(), cx, cy, ox, oy };
          scale.stop();
          zx.stop();
          zy.stop();
        }
        const k = s / m.s0;
        scale.set(s);
        zx.set(m.cx - (m.cx - m.x0) * k + (ox - m.ox));
        zy.set(m.cy - (m.cy - m.y0) * k + (oy - m.oy));
        if (s > HIRES_AT) setHiRes(true);
        if (last) {
          st.pinching = false;
          settleZoom();
          if (mobile && s > 1.05) setChrome(false);
        }
        return m;
      },
      onWheel: ({ event, delta: [, wy], first }) => {
        const e = event as WheelEvent;
        if (e.ctrlKey || item?.kind !== 'image' || !wy) return;
        if (first) scale.stop();
        const [cx, cy] = center(e.clientX, e.clientY);
        zoomTo(scale.get() * (wy < 0 ? 1.15 : 1 / 1.15), cx, cy, false);
      },
    },
    {
      target: stageRef,
      eventOptions: { passive: false },
      drag: { filterTaps: true, threshold: 3, pointer: { capture: false } },
      pinch: { scaleBounds: { min: 0.7, max: MAX_ZOOM }, rubberband: true, from: () => [scale.get(), 0] },
    },
  );

  // ── keyboard ──
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // (a focused native <audio> player seeks with the arrow keys itself)
      const tag = (e.target as HTMLElement).tagName;
      if (/input|textarea/i.test(tag) || (tag === 'AUDIO' && e.key !== 'Escape') || document.querySelector('[role=dialog]')) return;
      if (e.key === 'Escape') close();
      else if (e.key === 'ArrowRight') page(1);
      else if (e.key === 'ArrowLeft') page(-1);
      else if (e.key === 'i') setPrefs({ infoOpen: !usePrefs.getState().infoOpen });
      else if (e.key === 'f' && item?.kind === 'image') void document.documentElement.requestFullscreen?.();
      else if ((e.key === '+' || e.key === '=') && item?.kind === 'image') zoomBy(zoomStep);
      else if (e.key === '-' && item?.kind === 'image') zoomBy(1 / zoomStep);
      else if (e.key === '0') resetZoom();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [close, page, setPrefs, item?.kind, zoomTo, zoomBy, resetZoom, scale, zoomStep]);

  useEffect(() => {
    const el = strip.current?.querySelector<HTMLElement>(`[data-strip="${id}"]`);
    el?.scrollIntoView({ inline: 'center', block: 'nearest', behavior: 'smooth' });
  }, [id]);

  // lock page scroll behind
  useEffect(() => {
    document.body.dataset.viewer = '1';
    return () => {
      delete document.body.dataset.viewer;
    };
  }, []);

  // details (place) for the mobile title once the slide settles
  const [settled, setSettled] = useState(id);
  useEffect(() => {
    const tm = setTimeout(() => setSettled(id), 450);
    return () => clearTimeout(tm);
  }, [id]);
  const details = useQuery({ queryKey: ['media', backend.id, id], queryFn: () => backend.media(id), enabled: mobile && settled === id && !!item, staleTime: 60_000 });
  const place = !hideLocation && details.data?.place ? [details.data.place.city, details.data.place.admin1].filter(Boolean).join(', ') : null;

  const stripItems = useMemo(() => {
    if (!source || index < 0 || mobile || !filmstrip) return [];
    const out: { item: MediaItem; index: number }[] = [];
    for (let k = Math.max(0, index - 30); k <= Math.min(source.total - 1, index + 30); k++) {
      const it = source.getAt(k);
      if (viewable(it)) out.push({ item: it!, index: k });
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source, index, mobile, filmstrip, source?.version]);

  const slides: { item: MediaItem; role: -1 | 0 | 1 }[] = [];
  if (prev) slides.push({ item: prev.item, role: -1 });
  if (item) slides.push({ item, role: 0 });
  if (next) slides.push({ item: next.item, role: 1 });

  const list = item ? actions.for([item]) : [];
  const bottomIds = ['share', 'download', 'edit', 'favorite', 'delete'];
  const bottom = list.filter((a) => bottomIds.includes(a.id));
  const rest = list.filter((a) => !bottomIds.includes(a.id));
  const desktopTop = list.filter((a) => ['favorite', 'edit', 'download', 'share', 'delete'].includes(a.id));
  const slideshowFromHere = async () => {
    if (!item) return;
    const listItems = source && index >= 0 ? await source.itemsInRange(index, index + 1000) : [item];
    void actions.run('slideshow', listItems.filter(hasPicture));
  };
  const local = item ? item.localAt : 0;
  const showChrome = chrome && !closing;

  return (
    <motion.div className="fixed inset-0 z-[60] flex flex-col overflow-hidden text-white" onPointerMove={poke} initial={{ opacity: 1 }} exit={{ opacity: 1 }} data-viewer>
      <motion.div className="absolute inset-0" style={{ opacity: bgOpacity }}>
      <motion.div className={cn('absolute inset-0', viewerBg === 'tint' ? 'bg-[color-mix(in_oklab,var(--accent)_16%,rgb(6_6_9))]' : viewerBg === 'black' ? 'bg-black' : 'bg-[rgb(6_6_9)]')} initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0, transition: { duration: 0.28 } }} transition={{ duration: 0.3 }} />
      {/* ambient backdrop: upscaled thumbhash (naturally blurry — no CSS filter) */}
      {item?.thumbhash && viewerBg === 'blur' && (
        <motion.img
          key={`bg-${item.id}`}
          src={thumbhashUrl(item.thumbhash)}
          alt=""
          aria-hidden
          className="pointer-events-none absolute inset-0 size-full object-cover"
          initial={{ opacity: 0 }}
          animate={{ opacity: closing ? 0 : 0.28 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.5 }}
        />
      )}
      </motion.div>

      {/* stage */}
      <div className={cn('absolute transition-[right] duration-300', mobile ? 'inset-0' : cn('top-16 bottom-20 left-16', infoOpen ? 'right-[356px]' : 'right-16'))}>
        <div ref={stageRef} className="absolute inset-0 touch-none overflow-hidden select-none" style={{ touchAction: 'none' }}>
          <motion.div className="absolute inset-0" style={{ x: trackX }}>
            {slides.map((s) => (
              <motion.div
                key={s.item.id}
                className="absolute top-0 h-full"
                data-viewer-active={s.role === 0 ? '' : undefined}
                style={{ left: s.role * (size.W + GAP), width: size.W, y: s.role === 0 ? slideY : zero, scale: s.role === 0 ? slideScale : one }}
              >
                <Slide
                  item={s.item}
                  W={size.W}
                  H={size.H}
                  active={s.role === 0}
                  hero={s.role === 0 && activeId === s.item.id}
                  hiRes={s.role === 0 && hiRes}
                  zoom={s.role === 0 ? zoomVals : staticZoom}
                  paused={swiping}
                  barInset={mobile}
                />
              </motion.div>
            ))}
            {!item && (
              <div className="absolute inset-0 grid place-items-center text-sm text-white/60" style={{ width: size.W }}>
                {t('mobile.viewer.loading')}
              </div>
            )}
          </motion.div>
        </div>
      </div>

      {/* ── chrome ── */}
      {mobile ? (
        <>
          <MobileTopBar
            visible={showChrome}
            title={item ? (local ? formatLocal(local, lang, { day: 'numeric', month: 'long', year: 'numeric' }) : t('mobile.viewer.unknownDate')) : ''}
            subtitle={item ? [local ? formatLocal(local, lang, { hour: '2-digit', minute: '2-digit' }) : null, place].filter(Boolean).join(' · ') : ''}
            onBack={close}
            onMore={() => setMenuSheet(true)}
            t={t}
          />
          <motion.nav
            className="absolute inset-x-0 bottom-0 z-10 bg-gradient-to-t from-black/75 via-black/40 to-transparent pt-6 pb-[env(safe-area-inset-bottom)]"
            initial={false}
            animate={{ opacity: showChrome ? 1 : 0, y: showChrome ? 0 : 24 }}
            style={{ pointerEvents: showChrome ? 'auto' : 'none' }}
            data-no-gesture
          >
            <div className="flex items-stretch justify-around px-1 pb-1">
              {item &&
                mobileBarOrder(bottom, INFO).map((a) => (
                  <button
                    key={a.id}
                    type="button"
                    onClick={() => (a === INFO ? setInfoSheet(true) : void actions.run(a, [item]))}
                    className={cn(
                      'flex min-h-[56px] min-w-[60px] flex-1 flex-col items-center justify-center gap-1 rounded-xl text-[11.5px] font-medium active:bg-white/10 [&_svg]:size-[22px]',
                      a.active?.([item], actions.ctx) && 'text-[var(--accent-2)] [&_svg]:fill-current',
                    )}
                  >
                    {a === INFO ? <Info /> : iconOf(a, [item], actions.ctx)}
                    <span className="max-w-[76px] truncate">{a === INFO ? t('viewer.info') : shortLabelOf(a, [item], actions.ctx)}</span>
                  </button>
                ))}
            </div>
          </motion.nav>
          <BottomSheet open={menuSheet} onClose={() => setMenuSheet(false)} dark z={90}>
            {item && (
              <>
                <SheetItem
                  icon={<Info />}
                  onClick={() => {
                    setMenuSheet(false);
                    setInfoSheet(true);
                  }}
                >
                  {t('viewer.details')}
                </SheetItem>
                <SheetItem
                  icon={<Presentation />}
                  onClick={() => {
                    setMenuSheet(false);
                    void slideshowFromHere();
                  }}
                >
                  {t('viewer.slideshow')}
                </SheetItem>
                {rest
                  .filter((a) => a.id !== 'slideshow')
                  .map((a) => (
                    <SheetItem
                      key={a.id}
                      icon={iconOf(a, [item], actions.ctx)}
                      danger={a.destructive}
                      onClick={() => {
                        setMenuSheet(false);
                        void actions.run(a, [item]);
                      }}
                    >
                      {labelOf(a, [item], actions.ctx)}
                    </SheetItem>
                  ))}
              </>
            )}
          </BottomSheet>
          <BottomSheet open={infoSheet && !!item} onClose={() => setInfoSheet(false)} title={t('viewer.details')} dark z={90}>
            <div className="px-5 [--border:rgb(255_255_255/0.08)] [--text:#f5f5f7] [--text-dim:#a3a3b0] [--text-faint:#8a8a98]">{item && <InfoContent item={item} />}</div>
          </BottomSheet>
        </>
      ) : (
        <>
          <motion.div
            className="relative z-10 flex h-16 shrink-0 items-center gap-2 px-5"
            initial={{ opacity: 0, y: -20 }}
            animate={{ opacity: showChrome ? 1 : 0, y: showChrome ? 0 : -20 }}
            exit={{ opacity: 0, y: -20 }}
            data-no-gesture
          >
            <IconButton label={t('common.close')} tone="glass" onClick={close}>
              <X />
            </IconButton>
            {item && (
              <div className="ml-2 min-w-0 flex-1">
                <div className="truncate text-sm font-semibold">{item.name}</div>
                <div className="truncate text-xs text-white/60">
                  {local ? formatLocal(local, lang, { dateStyle: 'long', timeStyle: 'short' }) : formatDate(item.takenAt, lang, { dateStyle: 'long', timeStyle: 'short' })}
                  {source && index >= 0 && ` · ${t('mobile.viewer.position', { i: index + 1, n: source.total })}`}
                </div>
              </div>
            )}
            {item && (
              <div className="flex items-center gap-1.5">
                {item.kind === 'image' && (
                  <>
                    <IconButton label={t('viewer.zoomOut')} tone="glass" onClick={() => zoomBy(1 / zoomStep)}>
                      <ZoomOut />
                    </IconButton>
                    <IconButton label={t('viewer.zoomIn')} tone="glass" onClick={() => zoomBy(zoomStep)}>
                      <ZoomIn />
                    </IconButton>
                  </>
                )}
                {desktopTop.map((a) => (
                  <IconButton key={a.id} label={labelOf(a, [item], actions.ctx)} tone="glass" onClick={() => void actions.run(a, [item])}>
                    <span className={cn('contents', a.active?.([item], actions.ctx) && '[&_svg]:fill-[var(--accent-2)] [&_svg]:text-[var(--accent-2)]')}>{iconOf(a, [item], actions.ctx)}</span>
                  </IconButton>
                ))}
                <IconButton label={t('viewer.slideshow')} tone="glass" onClick={() => void slideshowFromHere()}>
                  {<Presentation />}
                </IconButton>
                {rest.filter((a) => a.id !== 'slideshow').length > 0 && <DesktopMore actions={rest.filter((a) => a.id !== 'slideshow')} item={item} run={(a) => void actions.run(a, [item])} ctx={actions.ctx} t={t} />}
                <IconButton label={t('viewer.info')} tone="glass" active={infoOpen} onClick={() => setPrefs({ infoOpen: !infoOpen })}>
                  <Info />
                </IconButton>
                <IconButton label={t('viewer.fullscreen')} tone="glass" onClick={() => (document.fullscreenElement ? document.exitFullscreen() : document.documentElement.requestFullscreen())}>
                  <Maximize />
                </IconButton>
              </div>
            )}
          </motion.div>
          {prev && (
            <motion.button
              type="button"
              aria-label={t('viewer.prev')}
              onClick={() => page(-1)}
              animate={{ opacity: showChrome && !zoomed ? 1 : 0 }}
              className="glass absolute top-1/2 left-3 z-10 grid size-12 -translate-y-1/2 place-items-center rounded-full border border-white/10 text-white hover:bg-white/15"
            >
              <ChevronLeft className="size-6" />
            </motion.button>
          )}
          {next && (
            <motion.button
              type="button"
              aria-label={t('viewer.next')}
              onClick={() => page(1)}
              animate={{ opacity: showChrome && !zoomed ? 1 : 0 }}
              className={cn('glass absolute top-1/2 right-3 z-10 grid size-12 -translate-y-1/2 place-items-center rounded-full border border-white/10 text-white hover:bg-white/15', infoOpen && 'right-[352px]')}
            >
              <ChevronRight className="size-6" />
            </motion.button>
          )}
          <AnimatePresence>{infoOpen && item && <InfoPanel item={item} onClose={() => setPrefs({ infoOpen: false })} />}</AnimatePresence>
          {stripItems.length > 1 && (
            <motion.div
              ref={strip}
              className={cn('no-scrollbar absolute inset-x-0 bottom-0 z-10 flex h-20 items-center gap-1.5 overflow-x-auto px-[50%] pb-3', infoOpen && 'right-[340px]')}
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: showChrome ? 1 : 0, y: showChrome ? 0 : 20 }}
              exit={{ opacity: 0, y: 20 }}
              data-no-gesture
            >
              {stripItems.map(({ item: m, index: k }) => (
                <button
                  key={m.id}
                  type="button"
                  data-strip={m.id}
                  aria-label={m.name}
                  aria-current={m.id === id ? 'true' : undefined}
                  onClick={() => goTo(m)}
                  className={cn('relative h-14 shrink-0 overflow-hidden rounded-lg transition-all duration-300', m.id === id ? 'w-20 opacity-100 ring-2 ring-white' : 'w-10 opacity-50 hover:opacity-90')}
                  title={String(k + 1)}
                >
                  {m.kind === 'audio' ? (
                    <span className="grid size-full place-items-center bg-white/10">
                      <Music className="size-5 text-white/70" />
                    </span>
                  ) : (
                    <img src={backend.thumbUrl(m.id, 320)} alt="" className="size-full object-cover" draggable={false} />
                  )}
                </button>
              ))}
            </motion.div>
          )}
        </>
      )}
    </motion.div>
  );
}

const INFO = { id: 'info' } as MediaAction;
function MobileTopBar({ visible, title, subtitle, onBack, onMore, t }: { visible: boolean; title: string; subtitle: string; onBack: () => void; onMore: () => void; t: ReturnType<typeof useT> }) {
  return (
    <motion.header
      className="absolute inset-x-0 top-0 z-10 flex items-center gap-1 bg-gradient-to-b from-black/70 via-black/35 to-transparent px-1 pt-[env(safe-area-inset-top)] pb-5"
      initial={false}
      animate={{ opacity: visible ? 1 : 0, y: visible ? 0 : -16 }}
      style={{ pointerEvents: visible ? 'auto' : 'none' }}
      data-no-gesture
    >
      <button type="button" aria-label={t('common.back')} onClick={onBack} className="grid size-12 place-items-center rounded-full active:bg-white/15">
        <ArrowLeft className="size-6" />
      </button>
      <div className="min-w-0 flex-1 px-1">
        <div className="truncate text-[15px] font-semibold">{title}</div>
        {subtitle && <div className="truncate text-xs text-white/70">{subtitle}</div>}
      </div>
      <button type="button" aria-label={t('mobile.viewer.more')} onClick={onMore} className="grid size-12 place-items-center rounded-full active:bg-white/15">
        <MoreVertical className="size-6" />
      </button>
    </motion.header>
  );
}

function DesktopMore({ actions, item, run, ctx, t }: { actions: MediaAction[]; item: MediaItem; run: (a: MediaAction) => void; ctx: ReturnType<typeof useMediaActions>['ctx']; t: ReturnType<typeof useT> }) {
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  return (
    <>
      <IconButton ref={anchor} label={t('mobile.viewer.more')} tone="glass" active={open} onClick={() => setOpen((o) => !o)}>
        <MoreVertical />
      </IconButton>
      <DesktopMenu open={open} onClose={() => setOpen(false)} anchor={anchor}>
        {actions.map((a) => (
          <MenuRow
            key={a.id}
            icon={iconOf(a, [item], ctx)}
            danger={a.destructive}
            onClick={() => {
              setOpen(false);
              run(a);
            }}
          >
            {labelOf(a, [item], ctx)}
          </MenuRow>
        ))}
      </DesktopMenu>
    </>
  );
}

