import { useCallback, useEffect, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { Camera, Gauge, Maximize, Pause, PictureInPicture2, Play, Repeat, Volume2, VolumeX } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatTime } from '@/lib/format';
import { toast } from '@/store/toasts';
import { MenuItem, Popover } from '@/components/ui';

const SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2];

/** Custom video player. For codecs the browser can't play it uses the server transcode stream with restart-on-seek. */
const COARSE = typeof window !== 'undefined' && window.matchMedia('(pointer: coarse)').matches;

export function VideoPlayer({
  item,
  autoPlay = true,
  onFrameSaved,
  poster,
  compact = COARSE,
}: {
  item: MediaItem;
  autoPlay?: boolean;
  onFrameSaved?: () => void;
  poster?: string;
  /** phones: fewer, larger controls; tap on the picture is left to the viewer */
  compact?: boolean;
}) {
  const t = useT();
  const backend = useBackend();
  const ref = useRef<HTMLVideoElement>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  const transcode = item.playback === 'transcode';
  const [offset, setOffset] = useState(0); // transcode: server-side start position
  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [dur, setDur] = useState(item.duration ?? 0);
  const [buffered, setBuffered] = useState(0);
  const [muted, setMuted] = useState(false);
  const [volume, setVolume] = useState(1);
  const [loop, setLoop] = useState(false);
  const [speed, setSpeed] = useState(1);
  const [hover, setHover] = useState<{ x: number; t: number } | null>(null);
  const [controls, setControls] = useState(true);
  const [speedOpen, setSpeedOpen] = useState(false);
  const speedBtn = useRef<HTMLButtonElement>(null);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const src = transcode ? backend.streamUrl(item.id, offset) : backend.fileUrl(item.id);
  const strip = item.duration ? backend.filmstripUrl(item.id, 20, 90) : null;

  const poke = useCallback(() => {
    setControls(true);
    if (hideTimer.current) clearTimeout(hideTimer.current);
    hideTimer.current = setTimeout(() => setControls(false), 2400);
  }, []);

  useEffect(() => {
    poke();
    return () => void (hideTimer.current && clearTimeout(hideTimer.current));
  }, [poke]);

  const toggle = () => {
    const v = ref.current;
    if (!v) return;
    if (v.paused) void v.play();
    else v.pause();
  };

  const seek = (s: number) => {
    const v = ref.current;
    if (!v) return;
    const target = Math.max(0, Math.min(dur || v.duration || 0, s));
    if (transcode) {
      setOffset(target);
      setTime(target);
    } else v.currentTime = target;
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (/input|textarea/i.test((e.target as HTMLElement).tagName)) return;
      if (e.key === ' ' || e.key === 'k') {
        e.preventDefault();
        toggle();
      } else if (e.key === 'j') seek(time - 10);
      else if (e.key === 'l') seek(time + 10);
      else if (e.key === 'm') setMuted((m) => !m);
      else if (e.key === 'f') void wrap.current?.requestFullscreen?.();
      poke();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  useEffect(() => {
    if (ref.current) {
      ref.current.muted = muted;
      ref.current.volume = volume;
      ref.current.playbackRate = speed;
    }
  }, [muted, volume, speed, src]);

  const pct = dur ? (time / dur) * 100 : 0;
  const barPos = (clientX: number) => {
    const r = bar.current!.getBoundingClientRect();
    return Math.max(0, Math.min(1, (clientX - r.left) / r.width));
  };

  const saveFrame = async () => {
    const v = ref.current;
    if (!v || !v.videoWidth) return;
    const c = document.createElement('canvas');
    c.width = v.videoWidth;
    c.height = v.videoHeight;
    c.getContext('2d')!.drawImage(v, 0, 0);
    const blob = await new Promise<Blob | null>((r) => c.toBlob(r, 'image/png'));
    if (!blob) return;
    try {
      await backend.saveEdit(blob, { folderId: item.folderId, dir: item.dir, name: `${item.name.replace(/\.[^.]+$/, '')}_${formatTime(time).replace(':', '-')}`, format: 'jpg', quality: 94 });
      toast.success(t('viewer.frameSaved'));
      onFrameSaved?.();
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    }
  };

  return (
    <div
      ref={wrap}
      className={cn('relative flex size-full items-center justify-center bg-black/0', !controls && playing && 'cursor-none')}
      onPointerMove={poke}
      onClick={(e) => {
        if (!compact && e.target === ref.current) toggle();
      }}
    >
      <video
        ref={ref}
        key={src}
        src={src}
        poster={poster}
        preload="metadata"
        autoPlay={autoPlay || offset > 0}
        playsInline
        loop={loop}
        crossOrigin="anonymous"
        className="max-h-full max-w-full rounded-lg shadow-2xl"
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onTimeUpdate={(e) => setTime(offset + e.currentTarget.currentTime)}
        onLoadedMetadata={(e) => {
          if (!transcode && Number.isFinite(e.currentTarget.duration)) setDur(e.currentTarget.duration);
        }}
        onProgress={(e) => {
          const v = e.currentTarget;
          if (v.buffered.length) setBuffered(offset + v.buffered.end(v.buffered.length - 1));
        }}
        onEnded={() => setPlaying(false)}
      />

      <AnimatePresence>
        {!playing && (
          <motion.button
            type="button"
            initial={{ opacity: 0, scale: 0.7 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 1.3 }}
            transition={{ type: 'spring', stiffness: 400, damping: 25 }}
            onClick={toggle}
            aria-label={t('viewer.play')}
            data-no-gesture
            className="absolute grid size-20 place-items-center rounded-full bg-black/45 text-white hover:bg-black/60"
          >
            <Play className="ml-1 size-9 fill-current" />
          </motion.button>
        )}
      </AnimatePresence>

      <motion.div
        animate={{ opacity: controls || !playing ? 1 : 0, y: controls || !playing ? 0 : 12 }}
        transition={{ duration: 0.25 }}
        className={cn(
          'absolute rounded-2xl bg-black/60 text-white',
          compact ? 'inset-x-2 bottom-2 px-3 pt-2 pb-1' : 'inset-x-4 bottom-4 px-4 pt-3 pb-2 [@media(pointer:fine)]:backdrop-blur-xl',
        )}
        onClick={(e) => e.stopPropagation()}
        data-no-gesture
      >
        {transcode && <div className="mb-1 text-[11px] text-white/60">{t('viewer.transcoding')}</div>}
        {/* scrub bar */}
        <div
          ref={bar}
          className={cn('group relative cursor-pointer touch-none', compact ? 'h-8' : 'h-5')}
          onPointerDown={(e) => {
            (e.target as HTMLElement).setPointerCapture(e.pointerId);
            seek(barPos(e.clientX) * dur);
          }}
          onPointerMove={(e) => {
            const p = barPos(e.clientX);
            setHover({ x: p * 100, t: p * dur });
            if (e.buttons === 1 && !transcode) seek(p * dur);
          }}
          onPointerUp={(e) => transcode && seek(barPos(e.clientX) * dur)}
          onPointerLeave={() => setHover(null)}
        >
          <div className="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 overflow-hidden rounded-full bg-white/20 transition-[height] group-hover:h-1.5">
            <div className="absolute inset-y-0 left-0 bg-white/30" style={{ width: `${dur ? (buffered / dur) * 100 : 0}%` }} />
            <div className="absolute inset-y-0 left-0 bg-accent-gradient" style={{ width: `${pct}%` }} />
          </div>
          <div className="absolute top-1/2 size-3.5 -translate-x-1/2 -translate-y-1/2 scale-0 rounded-full bg-white shadow transition-transform group-hover:scale-100" style={{ left: `${pct}%` }} />
          {hover && (
            <div className="pointer-events-none absolute bottom-6 -translate-x-1/2" style={{ left: `${Math.max(8, Math.min(92, hover.x))}%` }}>
              {strip && (
                <div
                  className="mb-1 h-[90px] w-40 overflow-hidden rounded-lg border border-white/20 bg-black bg-no-repeat shadow-xl"
                  style={{
                    backgroundImage: `url(${strip})`,
                    backgroundSize: '2000% 100%',
                    backgroundPosition: `${(Math.min(19, Math.floor((hover.t / Math.max(0.01, dur)) * 20)) / 19) * 100}% 0`,
                  }}
                />
              )}
              <div className="text-center text-[11px] font-medium tabular-nums">{formatTime(hover.t)}</div>
            </div>
          )}
        </div>
        <div className="mt-1 flex items-center gap-1">
          <button type="button" onClick={toggle} className="grid size-9 place-items-center rounded-lg hover:bg-white/10 [@media(pointer:coarse)]:size-11" aria-label={playing ? t('viewer.pause') : t('viewer.play')}>
            {playing ? <Pause className="size-5 fill-current" /> : <Play className="size-5 fill-current" />}
          </button>
          <button type="button" onClick={() => setMuted((m) => !m)} className="grid size-9 place-items-center rounded-lg hover:bg-white/10 [@media(pointer:coarse)]:size-11" aria-label={muted ? t('viewer.unmute') : t('viewer.mute')}>
            {muted || volume === 0 ? <VolumeX className="size-5" /> : <Volume2 className="size-5" />}
          </button>
          {!compact && (
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={muted ? 0 : volume}
              onChange={(e) => {
                setVolume(Number(e.target.value));
                setMuted(false);
              }}
              className="peek-range w-20"
              style={{ ['--fill' as string]: `${(muted ? 0 : volume) * 100}%` }}
            />
          )}
          <span className="ml-2 text-xs text-white/80 tabular-nums">
            {formatTime(time)} / {formatTime(dur)}
          </span>
          <div className="flex-1" />
          {!compact && (
            <>
            <button type="button" title={t('viewer.saveFrame')} onClick={saveFrame} className="grid size-9 place-items-center rounded-lg hover:bg-white/10 [@media(pointer:coarse)]:size-11">
              <Camera className="size-[18px]" />
            </button>
            <button ref={speedBtn} type="button" title={t('viewer.speed')} onClick={() => setSpeedOpen((o) => !o)} className="flex h-9 items-center gap-1 rounded-lg px-2 text-xs font-semibold hover:bg-white/10">
              <Gauge className="size-[18px]" />
              {speed !== 1 && `${speed}×`}
            </button>
            <button type="button" title={t('viewer.loop')} onClick={() => setLoop((l) => !l)} className={cn('grid size-9 place-items-center rounded-lg hover:bg-white/10 [@media(pointer:coarse)]:size-11', loop && 'text-[var(--accent-2)]')}>
              <Repeat className="size-[18px]" />
            </button>
            {'pictureInPictureEnabled' in document && (
              <button type="button" title={t('viewer.pip')} onClick={() => void ref.current?.requestPictureInPicture?.()} className="grid size-9 place-items-center rounded-lg hover:bg-white/10 [@media(pointer:coarse)]:size-11">
                <PictureInPicture2 className="size-[18px]" />
              </button>
            )}
            </>
          )}
          <button type="button" title={t('viewer.fullscreen')} onClick={() => (document.fullscreenElement ? document.exitFullscreen() : wrap.current?.requestFullscreen())} className="grid size-9 place-items-center rounded-lg hover:bg-white/10 [@media(pointer:coarse)]:size-11">
            <Maximize className="size-[18px]" />
          </button>
        </div>
      </motion.div>
      <Popover open={speedOpen} onClose={() => setSpeedOpen(false)} anchor={speedBtn} side="top" align="end" className="w-32">
        {SPEEDS.map((s) => (
          <MenuItem
            key={s}
            checked={s === speed}
            onClick={() => {
              setSpeed(s);
              setSpeedOpen(false);
            }}
          >
            {s}×
          </MenuItem>
        ))}
      </Popover>
    </div>
  );
}
