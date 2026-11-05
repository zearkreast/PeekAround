import { useEffect, useRef, useState, type ReactNode } from 'react';
import { motion, type TargetAndTransition } from 'motion/react';
import { Clapperboard, Download, FolderOutput, Music, Palette, Pause, Play, Shuffle, Sparkles, Timer, X } from 'lucide-react';
import type { SlideTransition } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useT, type MessageKey } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatDuration } from '@/lib/format';
import { useLibraries } from '@/lib/queries';
import { Button, ColorSwatches, Input, Segmented, Slider, Switch } from '@/components/ui';
import { FolderPicker } from '@/features/gallery/FolderPicker';
import { JobProgressCard, useJobRun } from '@/features/video-editor/JobProgress';
import { buildExportSpec, kenBurnsOn, outputDims, playlist, showLength, type Aspect, type Res } from './show';
import { useSlideshow } from './store';

export function Card({ icon, title, children, delay = 0, className }: { icon: ReactNode; title: string; children: ReactNode; delay?: number; className?: string }) {
  return (
    <motion.section
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: 'spring', stiffness: 260, damping: 28, delay }}
      className={cn('rounded-3xl border border-line bg-elev p-5', className)}
    >
      <h2 className="mb-4 flex items-center gap-2.5 font-display text-[15px] font-semibold">
        <span className="grid size-8 place-items-center rounded-xl bg-surface-2 text-accent [&>svg]:size-4">{icon}</span>
        {title}
      </h2>
      <div className="flex flex-col gap-4">{children}</div>
    </motion.section>
  );
}

function Line({ label, hint, children }: { label: ReactNode; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div className="min-w-0">
        <div className="text-sm">{label}</div>
        {hint && <div className="text-xs text-faint">{hint}</div>}
      </div>
      {children}
    </div>
  );
}

// ───────────────────────── Transition picker ─────────────────────────

const TRANSITIONS: { value: SlideTransition; label: MessageKey }[] = [
  { value: 'fade', label: 'slide.tr.fade' },
  { value: 'slide', label: 'slide.tr.slide' },
  { value: 'zoom', label: 'slide.tr.zoom' },
  { value: 'kenburns', label: 'slide.tr.kenburns' },
  { value: 'blur', label: 'slide.tr.blur' },
  { value: 'flip', label: 'slide.tr.flip' },
  { value: 'circle', label: 'slide.tr.circle' },
  { value: 'random', label: 'slide.tr.random' },
  { value: 'none', label: 'slide.tr.none' },
];

const A = 'linear-gradient(135deg,#6366f1,#a855f7)';
const B = 'linear-gradient(135deg,#f97316,#ec4899)';

/** A tiny looping demo of a transition (A → B). */
function TransitionThumb({ kind, live }: { kind: SlideTransition; live: boolean }) {
  const rep = { duration: 1.1, repeat: Infinity, repeatDelay: 0.7, ease: [0.65, 0, 0.35, 1] } as const;
  const anim: Record<string, TargetAndTransition> = {
    fade: { opacity: [0, 1] },
    slide: { x: ['100%', '0%'] },
    zoom: { scale: [1.8, 1], opacity: [0, 1] },
    kenburns: { opacity: [0, 1], scale: [1, 1.25] },
    blur: { opacity: [0, 1], filter: ['blur(6px)', 'blur(0px)'] },
    flip: { rotateY: [90, 0] },
    circle: { clipPath: ['circle(0% at 50% 50%)', 'circle(75% at 50% 50%)'] },
    random: { opacity: [0, 1], x: ['-60%', '0%'], rotate: [-12, 0] },
    none: { opacity: [0, 0, 1, 1] },
  };
  return (
    <span className="relative block h-8 w-12 overflow-hidden rounded-lg" style={{ background: A, perspective: 120 }}>
      {!live && <span className="absolute inset-0 opacity-90" style={{ background: B, clipPath: 'polygon(62% 0, 100% 0, 100% 100%, 38% 100%)' }} />}
      <motion.span
        className="absolute inset-0 rounded-lg"
        style={{ background: B }}
        initial={false}
        animate={live ? anim[kind] : { opacity: 0, x: '0%', scale: 1, rotateY: 0, rotate: 0, clipPath: 'circle(75% at 50% 50%)', filter: 'blur(0px)' }}
        transition={live ? rep : { duration: 0.2 }}
      />
      {kind === 'random' && <Shuffle className="absolute top-1/2 left-1/2 size-4 -translate-x-1/2 -translate-y-1/2 text-white drop-shadow" />}
    </span>
  );
}

function TransitionPicker() {
  const t = useT();
  const value = useSlideshow((s) => s.settings.transition);
  const setSettings = useSlideshow((s) => s.setSettings);
  const [hover, setHover] = useState<SlideTransition | null>(null);
  return (
    <div className="grid grid-cols-3 gap-1.5">
      {TRANSITIONS.map((x) => {
        const active = x.value === value;
        return (
          <button
            key={x.value}
            type="button"
            onClick={() => setSettings({ transition: x.value })}
            onPointerEnter={() => setHover(x.value)}
            onPointerLeave={() => setHover(null)}
            className={cn('relative flex flex-col items-center gap-1.5 rounded-2xl px-1 py-2.5 text-[11px] font-medium transition-colors', active ? 'text-fg' : 'text-faint hover:text-dim')}
            data-testid={`transition-${x.value}`}
          >
            {active && <motion.span layoutId="slide-tr" className="absolute inset-0 rounded-2xl border border-[color-mix(in_oklab,var(--accent)_55%,transparent)] bg-[color-mix(in_oklab,var(--accent)_12%,transparent)]" transition={{ type: 'spring', stiffness: 500, damping: 36 }} />}
            <span className="relative">
              <TransitionThumb kind={x.value} live={active || hover === x.value} />
            </span>
            <span className="relative">{t(x.label)}</span>
          </button>
        );
      })}
    </div>
  );
}

// ───────────────────────── Music ─────────────────────────

function MusicRow({ onPick }: { onPick: () => void }) {
  const t = useT();
  const backend = useBackend();
  const music = useSlideshow((s) => s.settings.music);
  const setSettings = useSlideshow((s) => s.setSettings);
  const [playing, setPlaying] = useState(false);
  const audio = useRef<HTMLAudioElement | null>(null);
  useEffect(() => {
    audio.current?.pause();
    audio.current = null;
    setPlaying(false);
  }, [music?.id]);
  useEffect(() => () => audio.current?.pause(), []);
  if (!music)
    return (
      <button
        type="button"
        onClick={onPick}
        className="flex items-center gap-3 rounded-2xl border border-dashed border-line-strong px-4 py-3 text-left text-sm text-dim transition-colors hover:border-[var(--accent)] hover:text-fg"
        data-testid="pick-music"
      >
        <Music className="size-5 text-accent" />
        <span className="flex-1">{t('slide.pickMusic')}</span>
      </button>
    );
  const toggle = () => {
    if (!audio.current) {
      audio.current = new Audio(backend.fileUrl(music.id));
      audio.current.onended = () => setPlaying(false);
    }
    if (playing) audio.current.pause();
    else void audio.current.play().catch(() => {});
    setPlaying(!playing);
  };
  return (
    <div className="flex items-center gap-3 rounded-2xl bg-surface p-2.5">
      <button type="button" onClick={toggle} aria-label={playing ? t('viewer.pause') : t('viewer.play')} className="relative grid size-10 shrink-0 place-items-center rounded-xl bg-accent-gradient text-white">
        {playing ? <Pause className="size-4 fill-current" /> : <Play className="ml-0.5 size-4 fill-current" />}
        {playing && <motion.span className="absolute inset-0 rounded-xl border-2 border-white/50" animate={{ scale: [1, 1.25], opacity: [0.8, 0] }} transition={{ duration: 1.2, repeat: Infinity }} />}
      </button>
      <button type="button" className="min-w-0 flex-1 text-left" onClick={onPick}>
        <div className="truncate text-sm font-medium" data-testid="music-name">
          {music.name}
        </div>
        <div className="text-xs text-faint tabular-nums">
          {formatDuration(music.duration)} · {t('slide.musicFade')}
        </div>
      </button>
      <button type="button" aria-label={t('common.remove')} onClick={() => setSettings({ music: null })} className="grid size-8 place-items-center rounded-lg text-faint hover:bg-surface-2 hover:text-fg">
        <X className="size-4" />
      </button>
    </div>
  );
}

const BACKGROUNDS = ['#000000', '#0b0b10', '#1e1b4b', '#3f3f46', '#f5f5f4', '#ffffff'];

export function SettingsPanel({ onPickMusic }: { onPickMusic: () => void }) {
  const t = useT();
  const s = useSlideshow((st) => st.settings);
  const setSettings = useSlideshow((st) => st.setSettings);
  return (
    <>
      <Card icon={<Sparkles />} title={t('slide.style')}>
        <TransitionPicker />
        <Slider
          label={t('slide.transitionDuration')}
          value={s.transitionDuration}
          min={0.2}
          max={2.5}
          step={0.1}
          format={(v) => `${v.toFixed(1)} ${t('slide.sec')}`}
          onChange={(transitionDuration) => setSettings({ transitionDuration })}
        />
        <Line label={t('slide.kenBurns')} hint={s.transition === 'kenburns' ? t('slide.kenBurnsOnByStyle') : t('slide.kenBurnsHint')}>
          <Switch checked={kenBurnsOn(s)} disabled={s.transition === 'kenburns'} onChange={(kenBurns) => setSettings({ kenBurns })} label={t('slide.kenBurns')} />
        </Line>
      </Card>
      <Card icon={<Timer />} title={t('slide.timing')} delay={0.04}>
        <Slider label={t('slide.perPhoto')} value={s.duration} min={1} max={15} step={0.5} format={(v) => `${v} ${t('slide.sec')}`} onChange={(duration) => setSettings({ duration })} />
        <Line label={t('slide.shuffle')} hint={t('slide.shuffleHint')}>
          <Switch checked={s.shuffle} onChange={(shuffle) => setSettings({ shuffle })} label={t('slide.shuffle')} />
        </Line>
        <Line label={t('slide.loop')} hint={t('slide.loopHint')}>
          <Switch checked={s.loop} onChange={(loop) => setSettings({ loop })} label={t('slide.loop')} />
        </Line>
      </Card>
      <Card icon={<Music />} title={t('slide.music')} delay={0.08}>
        <MusicRow onPick={onPickMusic} />
      </Card>
      <Card icon={<Palette />} title={t('slide.look')} delay={0.12}>
        <Segmented
          className="w-full [&>button]:flex-1"
          value={s.aspect}
          onChange={(aspect: Aspect) => setSettings({ aspect })}
          options={(['16:9', '9:16', '1:1', '4:5'] as Aspect[]).map((a) => ({ value: a, label: a }))}
        />
        <div className="flex flex-col gap-2">
          <span className="text-[13px] text-dim">{t('slide.background')}</span>
          <ColorSwatches value={s.background} onChange={(background) => setSettings({ background })} colors={BACKGROUNDS} />
        </div>
        <Line label={t('slide.captions')} hint={t('slide.captionsHint')}>
          <Switch checked={s.captions} onChange={(captions) => setSettings({ captions })} label={t('slide.captions')} />
        </Line>
      </Card>
    </>
  );
}

// ───────────────────────── Export ─────────────────────────

export function ExportCard() {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const libs = useLibraries();
  const items = useSlideshow((s) => s.items);
  const s = useSlideshow((st) => st.settings);
  const setSettings = useSlideshow((st) => st.setSettings);
  const target = useSlideshow((st) => st.target);
  const setTarget = useSlideshow((st) => st.setTarget);
  const [name, setName] = useState('');
  const [picking, setPicking] = useState(false);
  const run = useJobRun('slideshow');
  const dims = outputDims(s.res, s.aspect);
  const length = showLength(playlist(items, { ...s, shuffle: false }, () => 0), s.transition === 'none' ? 0 : s.transitionDuration);
  const dest = target ?? (items[0] ? { folderId: items[0].media.folderId, dir: items[0].media.dir } : null);
  const folder = libs.data?.libraries.flatMap((l) => l.folders.map((f) => ({ f, l }))).find((x) => x.f.id === dest?.folderId);
  const destLabel = dest ? [folder ? (folder.l.folders.length > 1 ? folder.f.name : folder.l.name) : t('slide.folder'), ...dest.dir.split('/').filter(Boolean)].join(' / ') : '—';
  const disabled = info?.caps.readonly ? t('settings.readonly') : info && !info.caps.slideshowExport ? t('video.noFfmpeg') : !items.length ? t('slide.emptyTitle') : null;

  const start = () =>
    run.start(async () => backend.startSlideshow(buildExportSpec(items, s, { target: dest, name: name || t('slide.defaultName') })));

  return (
    <Card icon={<Clapperboard />} title={t('slide.exportVideo')} delay={0.16}>
      <Segmented
        className="w-full [&>button]:flex-1"
        value={String(s.res)}
        onChange={(v) => setSettings({ res: Number(v) as Res })}
        options={[
          { value: '720', label: '720p' },
          { value: '1080', label: '1080p' },
          { value: '2160', label: '4K' },
        ]}
      />
      <div className="flex items-center justify-between text-xs text-faint tabular-nums">
        <span>
          {dims.w} × {dims.h} · 30 fps · MP4
        </span>
        <span>{formatDuration(length)}</span>
      </div>
      <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={t('slide.defaultName')} aria-label={t('common.name')} data-testid="export-name" />
      <button type="button" onClick={() => setPicking(true)} className="flex items-center gap-3 rounded-2xl bg-surface px-3 py-2.5 text-left hover:bg-surface-2">
        <FolderOutput className="size-4 shrink-0 text-accent" />
        <span className="min-w-0 flex-1">
          <span className="block text-xs text-faint">{t('slide.saveTo')}</span>
          <span className="block truncate text-sm">{destLabel}</span>
        </span>
        <span className="text-xs text-accent">{t('slide.change')}</span>
      </button>
      <Button variant="primary" size="lg" icon={<Download className="size-5" />} disabled={!!disabled} loading={run.busy} onClick={start} title={disabled ?? undefined} data-testid="slideshow-export">
        {t('slide.exportVideo')}
      </Button>
      {disabled && <p className="-mt-2 text-center text-xs text-faint">{disabled}</p>}
      <FolderPicker
        open={picking}
        title={t('slide.saveTo')}
        onClose={() => setPicking(false)}
        onPick={(p) => {
          setTarget({ folderId: p.folderId, dir: p.dir });
          setPicking(false);
        }}
      />
      <JobProgressCard run={run} kind="slideshow" title={`${name || t('slide.defaultName')}.mp4`} />
    </Card>
  );
}
