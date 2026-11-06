import { useCallback, useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router';
import { AnimatePresence, motion, useMotionValueEvent, type MotionValue } from 'motion/react';
import { Captions, ChevronLeft, ChevronRight, Maximize, Minimize, Music, Pause, Play, Presentation, RotateCcw, Volume2, VolumeX, X } from 'lucide-react';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatDuration } from '@/lib/format';
import { Button, EmptyState } from '@/components/ui';
import { playlist, showLength, type PlaySlide } from './show';
import { ShowStage, usePreload } from './ShowStage';
import { useSlideshow } from './store';
import { useShow, type ShowState } from './useShow';

function Segments({ n, index, progress }: { n: number; index: number; progress: MotionValue<number> }) {
  if (n > 40) return <OverallBar n={n} index={index} progress={progress} />;
  return (
    <div className="flex flex-1 gap-1">
      {Array.from({ length: n }, (_, i) => (
        <div key={i} className="h-1 flex-1 overflow-hidden rounded-full bg-white/20">
          {i < index && <div className="h-full w-full bg-white" />}
          {i === index && <motion.div className="h-full origin-left bg-white" style={{ scaleX: progress }} />}
        </div>
      ))}
    </div>
  );
}

/** One continuous bar for long shows. */
function OverallBar({ n, index, progress }: { n: number; index: number; progress: MotionValue<number> }) {
  const [v, setV] = useState(index / n);
  useMotionValueEvent(progress, 'change', (p) => setV((index + p) / n));
  return (
    <div className="h-1 flex-1 overflow-hidden rounded-full bg-white/20">
      <motion.div className="h-full origin-left bg-white" style={{ scaleX: v }} />
    </div>
  );
}

/** Background music with fade-out over the last 2 s (unless looping). */
function useMusic(url: string | null, show: ShowState, loop: boolean, muted: boolean) {
  const ref = useRef<HTMLAudioElement | null>(null);
  useEffect(() => {
    if (!url) return;
    const a = new Audio(url);
    a.loop = true;
    a.preload = 'auto';
    ref.current = a;
    return () => {
      a.pause();
      a.src = '';
      ref.current = null;
    };
  }, [url]);
  useEffect(() => {
    const a = ref.current;
    if (!a) return;
    if (show.playing) void a.play().catch(() => {});
    else a.pause();
  }, [show.playing, url]);
  useEffect(() => {
    if (ref.current) ref.current.muted = muted;
  }, [muted, url]);
  // replay after the end starts the music from the top again
  const wasEnded = useRef(false);
  useEffect(() => {
    if (wasEnded.current && !show.ended && ref.current) {
      ref.current.currentTime = 0;
      ref.current.volume = 1;
    }
    wasEnded.current = show.ended;
  }, [show.ended]);
  useMotionValueEvent(show.remaining, 'change', (r) => {
    const a = ref.current;
    if (!a) return;
    a.volume = loop ? 1 : Math.max(0, Math.min(1, r / 2));
  });
  return ref;
}

export function Player() {
  const t = useT();
  const nav = useNavigate();
  const backend = useBackend();
  const items = useSlideshow((s) => s.items);
  const settings = useSlideshow((s) => s.settings);
  const setSettings = useSlideshow((s) => s.setSettings);
  // resolve order / random transitions once per playback
  const [slides] = useState<PlaySlide[]>(() => playlist(items, settings));
  const td = settings.transition === 'none' ? 0 : settings.transitionDuration;
  const show = useShow(slides, td, { autoplay: true, loop: settings.loop });
  const [muted, setMuted] = useState(false);
  const [ui, setUi] = useState(true);
  const [full, setFull] = useState(!!document.fullscreenElement);
  const root = useRef<HTMLDivElement>(null);
  const hide = useRef<ReturnType<typeof setTimeout> | null>(null);
  useMusic(settings.music ? backend.fileUrl(settings.music.id) : null, show, settings.loop, muted);
  usePreload(slides, show.index, 1600);

  const exit = useCallback(() => {
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
    nav('/slideshow', { replace: true });
  }, [nav]);

  const poke = useCallback(() => {
    setUi(true);
    if (hide.current) clearTimeout(hide.current);
    hide.current = setTimeout(() => setUi(false), 2600);
  }, []);

  const toggleFull = useCallback(() => {
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
    else void root.current?.requestFullscreen?.().catch(() => {});
  }, []);

  useEffect(() => {
    poke();
    // the Play click is a user gesture, so fullscreen is usually allowed right away
    void root.current?.requestFullscreen?.().catch(() => {});
    const onFs = () => setFull(!!document.fullscreenElement);
    document.addEventListener('fullscreenchange', onFs);
    return () => {
      document.removeEventListener('fullscreenchange', onFs);
      if (hide.current) clearTimeout(hide.current);
    };
  }, [poke]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      switch (e.key) {
        case ' ':
        case 'k':
          e.preventDefault();
          show.toggle();
          break;
        case 'ArrowRight':
          show.next();
          break;
        case 'ArrowLeft':
          show.prev();
          break;
        case 'Escape':
          exit();
          break;
        case 'f':
        case 'F':
          toggleFull();
          break;
        case 'c':
        case 'C':
          setSettings({ captions: !useSlideshow.getState().settings.captions });
          break;
        case 'm':
        case 'M':
          setMuted((m) => !m);
          break;
        default:
          return;
      }
      poke();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [show, exit, toggleFull, poke, setSettings]);

  if (!slides.length)
    return (
      <div className="fixed inset-0 z-[60] grid place-items-center bg-bg">
        <EmptyState
          icon={<Presentation />}
          title={t('slide.emptyTitle')}
          body={t('slide.emptyBody')}
          action={
            <div className="flex gap-2">
              <Button variant="ghost" onClick={() => nav('/slideshow')}>
                {t('common.back')}
              </Button>
              <Button variant="primary" onClick={() => nav('/')}>
                {t('slide.goTimeline')}
              </Button>
            </div>
          }
        />
      </div>
    );

  const total = showLength(slides, td);
  const visible = ui || !show.playing;

  return (
    <div
      ref={root}
      className={cn('fixed inset-0 z-[60] overflow-hidden bg-black select-none', !visible && 'cursor-none')}
      onPointerMove={poke}
      onClick={(e) => {
        if (e.target === e.currentTarget || (e.target as HTMLElement).closest('[data-stage]')) {
          show.toggle();
          poke();
        }
      }}
      data-testid="slideshow-player"
    >
      <div data-stage className="absolute inset-0">
        <ShowStage
          slides={slides}
          index={show.index}
          dir={show.dir}
          playing={show.playing}
          clock={show.clock}
          startAt={show.startAt}
          td={td}
          background={settings.background}
          captions={settings.captions}
          className="size-full"
        />
      </div>

      {/* progress */}
      <motion.div
        className="absolute inset-x-0 top-0 z-20 flex items-center gap-3 bg-gradient-to-b from-black/55 to-transparent px-4 pt-3 pb-10 sm:px-6"
        animate={{ opacity: visible ? 1 : 0.35 }}
        transition={{ duration: 0.4 }}
      >
        <Segments n={slides.length} index={show.index} progress={show.progress} />
        <AnimatePresence>
          {visible && (
            <motion.button
              initial={{ opacity: 0, scale: 0.8 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.8 }}
              type="button"
              onClick={(e) => (e.stopPropagation(), exit())}
              aria-label={t('common.close')}
              className="grid size-9 place-items-center rounded-full bg-white/10 text-white backdrop-blur-xl hover:bg-white/20"
            >
              <X className="size-5" />
            </motion.button>
          )}
        </AnimatePresence>
      </motion.div>

      {/* big pause indicator */}
      <AnimatePresence>
        {!show.playing && !show.ended && (
          <motion.div
            initial={{ opacity: 0, scale: 0.6 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 1.4 }}
            transition={{ type: 'spring', stiffness: 380, damping: 24 }}
            className="pointer-events-none absolute top-1/2 left-1/2 z-20 grid size-20 -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full bg-black/35 text-white backdrop-blur-xl"
          >
            <Pause className="size-9 fill-current" />
          </motion.div>
        )}
      </AnimatePresence>

      {/* controls */}
      <AnimatePresence>
        {visible && !show.ended && (
          <motion.div
            initial={{ opacity: 0, y: 24 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 24 }}
            transition={{ type: 'spring', stiffness: 380, damping: 32 }}
            className="absolute bottom-5 left-1/2 z-20 flex -translate-x-1/2 items-center gap-1 rounded-2xl border border-white/10 bg-black/45 p-1.5 text-white shadow-2xl backdrop-blur-2xl"
            onClick={(e) => e.stopPropagation()}
          >
            <CtlButton label={t('viewer.prev')} onClick={show.prev}>
              <ChevronLeft />
            </CtlButton>
            <motion.button
              type="button"
              whileTap={{ scale: 0.88 }}
              onClick={show.toggle}
              aria-label={show.playing ? t('viewer.pause') : t('viewer.play')}
              className="grid size-11 place-items-center rounded-xl bg-white text-black"
              data-testid="player-toggle"
            >
              {show.playing ? <Pause className="size-5 fill-current" /> : <Play className="ml-0.5 size-5 fill-current" />}
            </motion.button>
            <CtlButton label={t('viewer.next')} onClick={show.next}>
              <ChevronRight />
            </CtlButton>
            <div className="mx-2 min-w-16 text-center text-xs text-white/80 tabular-nums">
              {show.index + 1} / {slides.length}
              <div className="text-[10px] text-white/50">{formatDuration(total)}</div>
            </div>
            <CtlButton label={t('slide.captions')} onClick={() => setSettings({ captions: !settings.captions })} active={settings.captions}>
              <Captions />
            </CtlButton>
            {settings.music && (
              <CtlButton label={muted ? t('viewer.unmute') : t('viewer.mute')} onClick={() => setMuted((m) => !m)} active={!muted}>
                {muted ? <VolumeX /> : <Volume2 />}
              </CtlButton>
            )}
            <CtlButton label={t('viewer.fullscreen')} onClick={toggleFull}>
              {full ? <Minimize /> : <Maximize />}
            </CtlButton>
          </motion.div>
        )}
      </AnimatePresence>

      {settings.music && visible && !show.ended && (
        <div className="pointer-events-none absolute bottom-6 left-6 z-20 hidden max-w-[28vw] items-center gap-2 truncate rounded-full bg-black/40 px-3 py-1.5 text-xs text-white/80 backdrop-blur-xl md:flex">
          <Music className="size-3.5 shrink-0" />
          <span className="truncate">{settings.music.name.replace(/\.[^.]+$/, '')}</span>
        </div>
      )}

      {/* end card */}
      <AnimatePresence>
        {show.ended && (
          <motion.div
            className="absolute inset-0 z-30 grid place-items-center bg-black/60 backdrop-blur-md"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={(e) => e.stopPropagation()}
          >
            <motion.div
              initial={{ opacity: 0, y: 24, scale: 0.94 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              transition={{ type: 'spring', stiffness: 300, damping: 26, delay: 0.15 }}
              className="flex flex-col items-center text-center text-white"
            >
              <div className="font-display text-4xl font-bold tracking-tight sm:text-5xl">
                <span className="text-gradient">{t('slide.theEnd')}</span>
              </div>
              <p className="mt-2 text-sm text-white/70">{t('slide.endBody', { n: slides.length, d: formatDuration(total) })}</p>
              <div className="mt-6 flex gap-2">
                <Button variant="glass" icon={<RotateCcw className="size-4" />} onClick={show.restart}>
                  {t('slide.replay')}
                </Button>
                <Button variant="primary" onClick={exit}>
                  {t('common.close')}
                </Button>
              </div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function CtlButton({ label, onClick, children, active }: { label: string; onClick: () => void; children: React.ReactNode; active?: boolean }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={cn('grid size-10 place-items-center rounded-xl transition-colors hover:bg-white/15 [&>svg]:size-5', active ? 'text-white' : 'text-white/70')}
    >
      {children}
    </button>
  );
}
