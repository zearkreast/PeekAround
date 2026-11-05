import { useEffect, useRef } from 'react';
import { AnimatePresence, motion, useTransform, type MotionValue, type Variants } from 'motion/react';
import { CalendarDays } from 'lucide-react';
import { useBackend } from '@/backend';
import { useLang } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatDate } from '@/lib/format';
import { cutLength, kbAt, type PlaySlide, type ShowTransition } from './show';

interface Custom {
  kind: ShowTransition;
  dir: number;
  d: number;
}

const EASE = [0.65, 0, 0.35, 1] as const;
/** Keep the outgoing slide on screen under the incoming one (identical keyframes would finish instantly). */
const HOLD = 0.995;

/**
 * Transition variants. `custom` always describes the cut into the *current* slide, so the
 * exiting slide (which receives the latest custom through AnimatePresence) animates in sync.
 */
const variants: Variants = {
  enter: ({ kind, dir }: Custom) => {
    switch (kind) {
      case 'none':
        return { opacity: 1, zIndex: 2 };
      case 'slide':
        return { x: `${100 * dir}%`, zIndex: 2, opacity: 1 };
      case 'zoom':
        return { scale: 1.45, opacity: 0, zIndex: 2 };
      case 'blur':
        return { opacity: 0, scale: 1.08, filter: 'blur(36px) brightness(1.5)', zIndex: 2 };
      case 'flip':
        return { rotateY: 90 * dir, scale: 0.86, opacity: 1, zIndex: 2 };
      case 'circle':
        return { clipPath: 'circle(0% at 50% 50%)', zIndex: 2, opacity: 1 };
      default:
        return { opacity: 0, zIndex: 2 };
    }
  },
  center: ({ kind, d }: Custom) => {
    const t = { duration: d, ease: EASE };
    switch (kind) {
      case 'none':
        return { opacity: 1, transition: { duration: 0 } };
      case 'slide':
        return { x: '0%', transition: t };
      case 'zoom':
        return { scale: 1, opacity: 1, transition: { duration: d, ease: [0.16, 1, 0.3, 1] } };
      case 'blur':
        return { opacity: 1, scale: 1, filter: 'blur(0px) brightness(1)', transition: t };
      case 'flip':
        return { rotateY: 0, scale: 1, transition: { duration: d / 2, delay: d / 2, ease: [0.22, 1, 0.36, 1] } };
      case 'circle':
        return { clipPath: 'circle(75% at 50% 50%)', transition: { duration: d, ease: [0.5, 0, 0.2, 1] } };
      default:
        return { opacity: 1, transition: t };
    }
  },
  exit: ({ kind, dir, d }: Custom) => {
    const t = { duration: d, ease: EASE };
    switch (kind) {
      case 'none':
        return { opacity: 0, transition: { duration: 0 } };
      case 'slide':
        return { x: `${-35 * dir}%`, filter: 'brightness(0.45)', zIndex: 1, transition: t };
      case 'zoom':
        return { scale: 0.9, opacity: [1, 0.3], zIndex: 1, transition: t };
      case 'blur':
        return { opacity: 0, filter: 'blur(36px) brightness(1.5)', zIndex: 1, transition: t };
      case 'flip':
        return { rotateY: -90 * dir, scale: 0.86, zIndex: 1, transition: { duration: d / 2, ease: [0.64, 0, 0.78, 0] } };
      default:
        return { opacity: HOLD, zIndex: 1, transition: { duration: d, ease: 'linear' } };
    }
  },
};

function SlideMedia({ slide, active, playing, size }: { slide: PlaySlide; active: boolean; playing: boolean; size: number }) {
  const backend = useBackend();
  const ref = useRef<HTMLVideoElement>(null);
  const m = slide.media;
  const transcode = m.playback === 'transcode';
  const start = slide.trim?.start ?? 0;
  useEffect(() => {
    const v = ref.current;
    if (!v) return;
    if (active && playing) void v.play().catch(() => {});
    else v.pause();
  }, [active, playing]);
  if (m.kind === 'video') {
    return (
      <video
        ref={ref}
        src={transcode ? backend.streamUrl(m.id, start) : `${backend.fileUrl(m.id)}#t=${start}`}
        muted
        playsInline
        preload="auto"
        poster={backend.thumbUrl(m.id, size)}
        onLoadedMetadata={(e) => {
          if (!transcode && Math.abs(e.currentTarget.currentTime - start) > 0.3) e.currentTarget.currentTime = start;
        }}
        className="size-full object-contain"
      />
    );
  }
  return <img src={backend.thumbUrl(m.id, size)} alt="" draggable={false} decoding="async" className="size-full object-contain select-none" />;
}

function Slide({
  slide,
  custom,
  active,
  playing,
  clock,
  start,
  span,
  size,
}: {
  slide: PlaySlide;
  custom: Custom;
  active: boolean;
  playing: boolean;
  clock: MotionValue<number>;
  start: number;
  span: number;
  size: number;
}) {
  const p = useTransform(clock, (c) => (c - start) / span);
  const scale = useTransform(p, (v) => (slide.kb >= 0 ? kbAt(slide.kb, v).scale : 1));
  const x = useTransform(p, (v) => (slide.kb >= 0 ? `${kbAt(slide.kb, v).x}%` : '0%'));
  const y = useTransform(p, (v) => (slide.kb >= 0 ? `${kbAt(slide.kb, v).y}%` : '0%'));
  return (
    <motion.div
      className="absolute inset-0 overflow-hidden [backface-visibility:hidden]"
      custom={custom}
      variants={variants}
      initial="enter"
      animate="center"
      exit="exit"
      style={{ background: 'var(--slide-bg)' }}
    >
      <motion.div className="size-full will-change-transform" style={{ scale, x, y }}>
        <SlideMedia slide={slide} active={active} playing={playing} size={size} />
      </motion.div>
    </motion.div>
  );
}

/** Renders the current slide with its transition (+ Ken Burns, captions). Sized by the parent. */
export function ShowStage({
  slides,
  index,
  dir,
  playing,
  clock,
  startAt,
  td,
  background,
  captions,
  size = 1600,
  className,
}: {
  slides: PlaySlide[];
  index: number;
  dir: number;
  playing: boolean;
  clock: MotionValue<number>;
  startAt: number;
  td: number;
  background: string;
  captions: boolean;
  size?: number;
  className?: string;
}) {
  const lang = useLang();
  const slide = slides[index];
  if (!slide) return null;
  const d = Math.max(0.05, cutLength(slides, index, td) || (slide.transition === 'none' ? 0 : Math.min(td, slide.dur / 2)));
  const custom: Custom = { kind: slide.transition, dir, d };
  const next = slides[index + 1];
  const span = slide.dur + (next ? cutLength(slides, index + 1, td) : 0);
  const name = slide.media.name.replace(/\.[^.]+$/, '');

  return (
    <div className={cn('relative overflow-hidden', className)} style={{ background, perspective: 1800, ['--slide-bg' as string]: background }}>
      <AnimatePresence initial={false} custom={custom}>
        <Slide key={`${slide.key}:${index}`} slide={slide} custom={custom} active playing={playing} clock={clock} start={startAt} span={Math.max(0.5, span)} size={size} />
      </AnimatePresence>
      {captions && (
        <AnimatePresence mode="wait">
          <motion.div
            key={slide.key}
            initial={{ opacity: 0, y: 14, filter: 'blur(6px)' }}
            animate={{ opacity: 1, y: 0, filter: 'blur(0px)', transition: { delay: d * 0.6, duration: 0.6, ease: [0.22, 1, 0.36, 1] } }}
            exit={{ opacity: 0, y: -6, transition: { duration: 0.25 } }}
            className="pointer-events-none absolute bottom-[8%] left-[5%] z-10 max-w-[80%] text-white [text-shadow:0_2px_18px_rgb(0_0_0/0.65)]"
          >
            <div className="truncate font-display text-[clamp(16px,3.2vmin,40px)] font-semibold tracking-tight">{name}</div>
            <div className="mt-1 flex items-center gap-1.5 text-[clamp(11px,1.7vmin,20px)] text-white/80">
              <CalendarDays className="size-[1em]" />
              {formatDate(slide.media.takenAt, lang, { dateStyle: 'long' })}
            </div>
          </motion.div>
        </AnimatePresence>
      )}
    </div>
  );
}

/** Warm the browser cache for the next slides. */
export function usePreload(slides: PlaySlide[], index: number, size: number) {
  const backend = useBackend();
  useEffect(() => {
    for (let k = 1; k <= 2; k++) {
      const s = slides[(index + k) % Math.max(1, slides.length)];
      if (!s || s === slides[index]) continue;
      const img = new Image();
      img.decoding = 'async';
      img.src = backend.thumbUrl(s.media.id, size);
      void img.decode?.().catch(() => {});
    }
  }, [slides, index, size, backend]);
}
