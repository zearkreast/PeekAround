import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { motion } from 'motion/react';
import { ChevronFirst, ChevronLast, Pause, Play, SkipBack, SkipForward, Type, Volume2, VolumeX } from 'lucide-react';
import type { MediaDetails } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { Kbd } from '@/components/ui';
import { clamp, outDuration, outToSrc, srcToOut, textWindow, type Size } from './model';
import { useEditor, usePlayhead } from './store';
import type { Playback } from './usePlayback';

const MIN_TRIM = 0.2;

export function fmtTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) s = 0;
  const m = Math.floor(s / 60);
  const sec = s - m * 60;
  return `${m}:${sec.toFixed(1).padStart(4, '0')}`;
}

function useWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [size, setSize] = useState<Size>({ w: 0, h: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setSize({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, size] as const;
}

function TransportButton({ label, onClick, children, active, className }: { label: string; onClick: () => void; children: ReactNode; active?: boolean; className?: string }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className={cn(
        'grid size-9 shrink-0 place-items-center rounded-xl transition-[background,color,transform] active:scale-90 [&>svg]:size-[18px]',
        active ? 'bg-surface-3 text-fg' : 'text-dim hover:bg-surface-2 hover:text-fg',
        className,
      )}
    >
      {children}
    </button>
  );
}

/** Play / step / time readout / set-in-out row above the timeline. */
export function Transport({ pb }: { pb: Playback }) {
  const t = useT();
  const playing = usePlayhead((s) => s.playing);
  const trim = useEditor((s) => s.s.trim);
  const speed = useEditor((s) => s.s.speed);
  const mute = useEditor((s) => s.s.mute);
  const set = useEditor((s) => s.set);
  const hasAudio = useEditor((s) => s.hasAudio);
  const outT = usePlayhead((s) => Math.round(clamp(srcToOut({ trim, speed }, s.time), 0, Number.MAX_VALUE) * 10) / 10);
  const dur = outDuration({ trim, speed });
  const setIn = () => {
    const now = usePlayhead.getState().time;
    if (now < trim.end - MIN_TRIM) set({ trim: { ...trim, start: now } });
  };
  const setOut = () => {
    const now = usePlayhead.getState().time;
    if (now > trim.start + MIN_TRIM) set({ trim: { ...trim, end: now } });
  };
  return (
    <div className="flex items-center gap-1 px-3 pt-2 sm:px-4">
      <TransportButton label={t('video.stepBack')} onClick={() => pb.step(-1)}>
        <SkipBack />
      </TransportButton>
      <motion.button
        type="button"
        whileTap={{ scale: 0.88 }}
        onClick={pb.toggle}
        aria-label={playing ? t('viewer.pause') : t('viewer.play')}
        className="grid size-11 place-items-center rounded-2xl bg-accent-gradient text-white shadow-[0_6px_20px_-6px_var(--accent)]"
        data-testid="play-toggle"
      >
        {playing ? <Pause className="size-5 fill-current" /> : <Play className="ml-0.5 size-5 fill-current" />}
      </motion.button>
      <TransportButton label={t('video.stepForward')} onClick={() => pb.step(1)}>
        <SkipForward />
      </TransportButton>
      <div className="ml-2 font-mono text-[13px] whitespace-nowrap text-dim tabular-nums">
        <span className="text-fg">{fmtTime(Math.min(outT, dur))}</span> / {fmtTime(dur)}
      </div>
      <div className="flex-1" />
      <TransportButton label={`${t('video.setIn')} (I)`} onClick={setIn}>
        <ChevronFirst />
      </TransportButton>
      <TransportButton label={`${t('video.setOut')} (O)`} onClick={setOut}>
        <ChevronLast />
      </TransportButton>
      {hasAudio && (
        <TransportButton label={mute ? t('viewer.unmute') : t('viewer.mute')} onClick={() => set({ mute: !mute })} active={mute}>
          {mute ? <VolumeX /> : <Volume2 />}
        </TransportButton>
      )}
      <span className="ml-1 hidden items-center gap-1 text-[11px] text-faint xl:flex">
        <Kbd>␣</Kbd> <Kbd>I</Kbd> <Kbd>O</Kbd>
      </span>
    </div>
  );
}

function Playhead({ duration, width }: { duration: number; width: number }) {
  const x = usePlayhead((s) => (duration ? (s.time / duration) * width : 0));
  return (
    <div className="pointer-events-none absolute -top-2 bottom-0 z-20" style={{ transform: `translateX(${x}px)` }}>
      <div className="absolute -top-0.5 -left-[7px] h-3 w-3.5 rounded-[4px] bg-white shadow-[0_2px_6px_rgb(0_0_0/0.5)]" />
      <div className="absolute top-2 bottom-0 -left-px w-0.5 rounded-full bg-white shadow-[0_0_8px_rgb(0_0_0/0.6)]" />
    </div>
  );
}

function Ruler({ duration, width }: { duration: number; width: number }) {
  if (!width || !duration) return <div className="h-5" />;
  const pps = width / duration;
  const step = [0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600].find((s) => s * pps >= 64) ?? 600;
  const ticks: number[] = [];
  for (let s = 0; s <= duration + 1e-6; s += step) ticks.push(s);
  return (
    <div className="relative h-5 text-[10px] text-faint tabular-nums">
      {ticks.map((s) => (
        <div key={s} className="absolute top-0 flex h-full items-start" style={{ left: s * pps }}>
          <div className="h-1.5 w-px bg-line-strong" />
          <span className="ml-1">{fmtTime(s).replace(/\.0$/, '')}</span>
        </div>
      ))}
    </div>
  );
}

type DragKind = { kind: 'scrub' } | { kind: 'in' } | { kind: 'out' } | { kind: 'text'; id: string; part: 'move' | 'start' | 'end'; x0: number; start: number; end: number };

/** Filmstrip + trim handles + playhead + text layer bars. */
export function Timeline({ item, pb }: { item: MediaDetails; pb: Playback }) {
  const t = useT();
  const backend = useBackend();
  const src = useEditor((s) => s.src);
  const duration = useEditor((s) => s.duration);
  const trim = useEditor((s) => s.s.trim);
  const speed = useEditor((s) => s.s.speed);
  const texts = useEditor((s) => s.s.texts);
  const selected = useEditor((s) => s.selectedText);
  const set = useEditor((s) => s.set);
  const select = useEditor((s) => s.select);
  const setTool = useEditor((s) => s.setTool);
  const [ref, size] = useWidth<HTMLDivElement>();
  const [loaded, setLoaded] = useState<string | null>(null);
  const drag = useRef<DragKind | null>(null);
  const [active, setActive] = useState<DragKind['kind'] | null>(null);

  const H = 64;
  const W = size.w;
  const n = W ? clamp(Math.round(W / (H * (src.w / src.h)) / 2) * 2, 4, 60) : 0;
  const strip = n ? backend.filmstripUrl(item.id, n, 120) : null;
  const pps = duration ? W / duration : 0;
  const dur = outDuration({ trim, speed });

  const timeAt = (clientX: number) => {
    const r = ref.current!.getBoundingClientRect();
    return clamp(((clientX - r.left) / r.width) * duration, 0, duration);
  };

  const begin = (d: DragKind) => (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    ref.current!.setPointerCapture(e.pointerId);
    drag.current = d;
    setActive(d.kind);
    if (d.kind === 'scrub') pb.scrub(timeAt(e.clientX));
    if (d.kind === 'text') {
      select(d.id);
      setTool('text');
    }
  };

  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const tm = timeAt(e.clientX);
    const s = useEditor.getState().s;
    if (d.kind === 'scrub') pb.scrub(tm);
    else if (d.kind === 'in') {
      const start = clamp(tm, 0, s.trim.end - MIN_TRIM);
      set({ trim: { ...s.trim, start } });
      pb.scrub(start);
    } else if (d.kind === 'out') {
      const end = clamp(tm, s.trim.start + MIN_TRIM, duration);
      set({ trim: { ...s.trim, end } });
      pb.scrub(end);
    } else {
      const total = outDuration(s);
      const delta = (e.clientX - d.x0) / pps / s.speed;
      let { start, end } = d;
      if (d.part === 'move') {
        const len = end - start;
        start = clamp(start + delta, 0, Math.max(0, total - len));
        end = start + len;
      } else if (d.part === 'start') start = clamp(start + delta, 0, end - 0.1);
      else end = clamp(end + delta, start + 0.1, total);
      set({ texts: s.texts.map((l) => (l.id === d.id ? { ...l, start, end } : l)) });
    }
  };

  const onUp = () => {
    const d = drag.current;
    drag.current = null;
    setActive(null);
    if (!d) return;
    if (d.kind === 'scrub' || d.kind === 'in') pb.endScrub();
    else if (d.kind === 'out') pb.endScrub(Math.max(useEditor.getState().s.trim.start, useEditor.getState().s.trim.end - 0.05));
  };

  const left = trim.start * pps;
  const right = trim.end * pps;

  return (
    <div className="px-7 pt-1 pb-3 sm:px-8" data-testid="timeline">
      <div ref={ref} className="relative select-none" onPointerMove={onMove} onPointerUp={onUp} onPointerCancel={onUp}>
        <Ruler duration={duration} width={W} />
        {/* filmstrip */}
        <div className="relative cursor-pointer touch-none" style={{ height: H }} onPointerDown={begin({ kind: 'scrub' })}>
          <div className={cn('absolute inset-0 overflow-hidden rounded-xl bg-surface-2', loaded !== strip && 'shimmer')}>
            {strip && (
              <img
                src={strip}
                alt=""
                draggable={false}
                onLoad={() => setLoaded(strip)}
                className={cn('size-full object-fill transition-opacity duration-500', loaded === strip ? 'opacity-100' : 'opacity-0')}
              />
            )}
          </div>
          {/* dimmed outside the trim range */}
          <div className="pointer-events-none absolute inset-y-0 left-0 rounded-l-xl bg-black/65" style={{ width: left }} />
          <div className="pointer-events-none absolute inset-y-0 right-0 rounded-r-xl bg-black/65" style={{ left: right }} />
          {/* selection frame */}
          <div className="pointer-events-none absolute inset-y-0 border-y-[3px] border-[var(--accent)]" style={{ left, width: Math.max(0, right - left) }} />
          {(['in', 'out'] as const).map((k) => (
            <div
              key={k}
              role="slider"
              aria-label={k === 'in' ? t('video.trimStart') : t('video.trimEnd')}
              aria-valuenow={k === 'in' ? trim.start : trim.end}
              onPointerDown={begin({ kind: k })}
              className={cn(
                'group absolute inset-y-0 z-10 flex w-4 cursor-ew-resize touch-none items-center justify-center bg-[var(--accent)] shadow-[0_0_12px_rgb(0_0_0/0.4)]',
                k === 'in' ? '-translate-x-full rounded-l-xl' : 'rounded-r-xl',
              )}
              style={{ left: k === 'in' ? left : right }}
              data-testid={`trim-${k}`}
            >
              <div className="h-6 w-1 rounded-full bg-white/90 transition-transform group-hover:scale-y-125" />
              {active === k && (
                <div className="absolute -top-8 rounded-lg bg-black/75 px-2 py-0.5 text-[11px] font-medium whitespace-nowrap text-white tabular-nums">
                  {fmtTime(k === 'in' ? trim.start : trim.end)}
                </div>
              )}
            </div>
          ))}
          <Playhead duration={duration} width={W} />
        </div>

        {/* text layer lanes (output timeline mapped back onto the source) */}
        {texts.length > 0 && (
          <div className="mt-2 flex max-h-[92px] flex-col gap-1 overflow-y-auto pr-0.5">
            {texts.map((l) => {
              const w = textWindow(l, dur);
              const x0 = outToSrc({ trim, speed }, w.start) * pps;
              const x1 = outToSrc({ trim, speed }, w.end) * pps;
              const sel = l.id === selected;
              return (
                <div key={l.id} className="relative h-6">
                  <motion.div
                    layout="position"
                    className={cn(
                      'absolute inset-y-0 flex cursor-grab touch-none items-center gap-1.5 overflow-hidden rounded-lg px-2 text-[11px] font-medium whitespace-nowrap active:cursor-grabbing',
                      sel ? 'bg-accent-gradient text-white shadow-[0_4px_14px_-4px_var(--accent)]' : 'bg-surface-3 text-dim hover:text-fg',
                    )}
                    style={{ left: x0, width: Math.max(14, x1 - x0) }}
                    onPointerDown={(e) => begin({ kind: 'text', id: l.id, part: 'move', x0: e.clientX, start: w.start, end: w.end })(e)}
                    data-testid="text-bar"
                  >
                    <Type className="size-3 shrink-0" />
                    <span className="truncate" style={{ fontFamily: `"${l.fontFamily}"` }}>
                      {l.text.split('\n')[0] || '…'}
                    </span>
                    {(['start', 'end'] as const).map((part) => (
                      <span
                        key={part}
                        className={cn('absolute inset-y-0 w-2 cursor-ew-resize hover:bg-white/30', part === 'start' ? 'left-0' : 'right-0')}
                        onPointerDown={(e) => begin({ kind: 'text', id: l.id, part, x0: e.clientX, start: w.start, end: w.end })(e)}
                      />
                    ))}
                  </motion.div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
