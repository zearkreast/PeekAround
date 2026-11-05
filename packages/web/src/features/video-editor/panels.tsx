import type { ReactNode } from 'react';
import { motion } from 'motion/react';
import {
  Contrast,
  Crop,
  Droplet,
  FlipHorizontal2,
  FlipVertical2,
  Palette,
  RotateCcw,
  RotateCw,
  Sun,
  SunDim,
  Thermometer,
  Undo2,
} from 'lucide-react';
import type { VideoAdjust } from '@peekaround/shared';
import { useT, type MessageKey } from '@/i18n';
import { cn } from '@/lib/cn';
import { Button, Slider, Switch } from '@/components/ui';
import {
  ASPECT_RATIOS,
  NO_ADJUST,
  SPEEDS,
  baseSize,
  clamp,
  exportSize,
  fitAspect,
  outDuration,
  rotated,
  unorientRect,
  type AspectKey,
  type Rotation,
} from './model';
import { useEditor, usePlayhead } from './store';
import { fmtTime } from './Timeline';

export function PanelSection({ title, hint, children, action }: { title: ReactNode; hint?: ReactNode; children: ReactNode; action?: ReactNode }) {
  return (
    <section className="flex flex-col gap-3 border-b border-line px-4 py-4 last:border-b-0">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h3 className="text-[13px] font-semibold tracking-wide text-fg">{title}</h3>
          {hint && <p className="mt-0.5 text-xs text-faint">{hint}</p>}
        </div>
        {action}
      </div>
      {children}
    </section>
  );
}

function NumberField({ label, value, onChange, min, max, step = 0.1 }: { label: string; value: number; onChange: (v: number) => void; min: number; max: number; step?: number }) {
  return (
    <label className="flex flex-1 flex-col gap-1">
      <span className="text-xs text-faint">{label}</span>
      <input
        type="number"
        value={Number(value.toFixed(2))}
        min={min}
        max={max}
        step={step}
        onChange={(e) => {
          const v = Number(e.target.value);
          if (Number.isFinite(v)) onChange(clamp(v, min, max));
        }}
        className="h-9 w-full rounded-xl border border-line bg-surface px-3 font-mono text-sm tabular-nums outline-none focus:border-[color-mix(in_oklab,var(--accent)_60%,transparent)]"
      />
    </label>
  );
}

// ───────────────────────── Trim ─────────────────────────

export function TrimPanel() {
  const t = useT();
  const duration = useEditor((s) => s.duration);
  const trim = useEditor((s) => s.s.trim);
  const set = useEditor((s) => s.set);
  const len = trim.end - trim.start;
  return (
    <PanelSection
      title={t('video.trim')}
      hint={t('video.trimHint')}
      action={
        (trim.start > 0 || trim.end < duration) && (
          <Button size="sm" variant="ghost" icon={<Undo2 className="size-4" />} onClick={() => set({ trim: { start: 0, end: duration } })}>
            {t('common.reset')}
          </Button>
        )
      }
    >
      <div className="flex gap-2">
        <NumberField label={t('video.trimStart')} value={trim.start} min={0} max={Math.max(0, trim.end - 0.2)} onChange={(start) => set({ trim: { ...trim, start } })} />
        <NumberField label={t('video.trimEnd')} value={trim.end} min={trim.start + 0.2} max={duration} onChange={(end) => set({ trim: { ...trim, end } })} />
      </div>
      <div className="flex gap-2">
        <Button size="sm" variant="secondary" className="flex-1" onClick={() => set({ trim: { ...trim, start: Math.min(usePlayhead.getState().time, trim.end - 0.2) } })}>
          {t('video.setIn')}
        </Button>
        <Button size="sm" variant="secondary" className="flex-1" onClick={() => set({ trim: { ...trim, end: Math.max(usePlayhead.getState().time, trim.start + 0.2) } })}>
          {t('video.setOut')}
        </Button>
      </div>
      <div className="flex items-center justify-between rounded-xl bg-surface px-3 py-2 text-[13px]">
        <span className="text-dim">{t('video.clipLength')}</span>
        <span className="font-mono tabular-nums">
          {fmtTime(len)} <span className="text-faint">/ {fmtTime(duration)}</span>
        </span>
      </div>
    </PanelSection>
  );
}

// ───────────────────────── Crop & rotate ─────────────────────────

const ASPECTS: { key: AspectKey; label: string }[] = [
  { key: 'free', label: '' },
  { key: '16:9', label: '16:9' },
  { key: '9:16', label: '9:16' },
  { key: '1:1', label: '1:1' },
  { key: '4:5', label: '4:5' },
];

const SWAP: Partial<Record<AspectKey, AspectKey>> = { '16:9': '9:16', '9:16': '16:9' };

export function CropPanel() {
  const t = useT();
  const src = useEditor((s) => s.src);
  const st = useEditor((s) => s.s);
  const set = useEditor((s) => s.set);
  const oriented = rotated(st.rotate) ? { w: src.h, h: src.w } : src;
  const out = exportSize(src, st);

  const pickAspect = (key: AspectKey) => {
    if (key === 'free') return set({ aspect: 'free' });
    const r = fitAspect(oriented, ASPECT_RATIOS[key]);
    set({ aspect: key, crop: unorientRect(r, src, st.rotate, st.flipH, st.flipV) });
  };
  const rotate = (d: 90 | -90) => {
    const r = (((st.rotate + d) % 360) + 360) % 360;
    const aspect = st.aspect === 'free' || st.aspect === '1:1' ? st.aspect : (SWAP[st.aspect] ?? 'free');
    set({ rotate: r as Rotation, aspect });
  };

  return (
    <>
      <PanelSection
        title={t('video.cropRatio')}
        hint={t('video.cropHint')}
        action={
          st.crop && (
            <Button size="sm" variant="ghost" icon={<Undo2 className="size-4" />} onClick={() => set({ crop: null, aspect: 'free' })}>
              {t('common.reset')}
            </Button>
          )
        }
      >
        <div className="grid grid-cols-5 gap-1.5">
          {ASPECTS.map((a) => {
            const r = a.key === 'free' ? 1.25 : ASPECT_RATIOS[a.key];
            const active = st.aspect === a.key;
            return (
              <button
                key={a.key}
                type="button"
                onClick={() => pickAspect(a.key)}
                className={cn('relative flex flex-col items-center gap-1.5 rounded-xl px-1 py-2.5 text-[11px] font-medium transition-colors', active ? 'text-fg' : 'text-faint hover:bg-surface-2 hover:text-dim')}
                data-testid={`aspect-${a.key}`}
              >
                {active && <motion.span layoutId="ve-aspect" className="absolute inset-0 rounded-xl bg-surface-3" transition={{ type: 'spring', stiffness: 500, damping: 36 }} />}
                <span className="relative grid h-6 place-items-center">
                  {a.key === 'free' ? (
                    <Crop className="size-5" />
                  ) : (
                    <span className={cn('block rounded-[4px] border-2', active ? 'border-[var(--accent)]' : 'border-current')} style={{ width: r >= 1 ? 22 : 22 * r, height: r >= 1 ? 22 / r : 22 }} />
                  )}
                </span>
                <span className="relative">{a.key === 'free' ? t('video.free') : a.label}</span>
              </button>
            );
          })}
        </div>
      </PanelSection>
      <PanelSection title={t('video.rotateFlip')}>
        <div className="grid grid-cols-4 gap-1.5">
          <ToolButton label={t('video.rotateLeft')} onClick={() => rotate(-90)} icon={<RotateCcw />} />
          <ToolButton label={t('video.rotateRight')} onClick={() => rotate(90)} icon={<RotateCw />} testId="rotate-right" />
          <ToolButton label={t('video.flipH')} onClick={() => set({ flipH: !st.flipH })} icon={<FlipHorizontal2 />} active={st.flipH} />
          <ToolButton label={t('video.flipV')} onClick={() => set({ flipV: !st.flipV })} icon={<FlipVertical2 />} active={st.flipV} />
        </div>
        <div className="flex items-center justify-between rounded-xl bg-surface px-3 py-2 text-[13px]">
          <span className="text-dim">{t('video.outputSize')}</span>
          <span className="font-mono tabular-nums">
            {out.w} × {out.h}
          </span>
        </div>
      </PanelSection>
    </>
  );
}

function ToolButton({ label, icon, onClick, active, testId }: { label: string; icon: ReactNode; onClick: () => void; active?: boolean; testId?: string }) {
  return (
    <motion.button
      type="button"
      whileTap={{ scale: 0.9 }}
      onClick={onClick}
      title={label}
      data-testid={testId}
      className={cn(
        'flex flex-col items-center gap-1.5 rounded-xl border px-1 py-2.5 text-[11px] font-medium transition-colors [&>svg]:size-5',
        active ? 'border-[color-mix(in_oklab,var(--accent)_60%,transparent)] bg-[color-mix(in_oklab,var(--accent)_14%,transparent)] text-fg' : 'border-line text-dim hover:bg-surface-2 hover:text-fg',
      )}
    >
      {icon}
      <span className="max-w-full truncate">{label}</span>
    </motion.button>
  );
}

// ───────────────────────── Adjust ─────────────────────────

const ADJUST: { key: keyof VideoAdjust; label: MessageKey; icon: ReactNode; min: number; max: number; unit: '%' | '°' }[] = [
  { key: 'brightness', label: 'video.brightness', icon: <Sun />, min: -1, max: 1, unit: '%' },
  { key: 'contrast', label: 'video.contrast', icon: <Contrast />, min: -1, max: 1, unit: '%' },
  { key: 'saturation', label: 'video.saturation', icon: <Droplet />, min: -1, max: 1, unit: '%' },
  { key: 'hue', label: 'video.hue', icon: <Palette />, min: -180, max: 180, unit: '°' },
  { key: 'warmth', label: 'video.warmth', icon: <Thermometer />, min: -1, max: 1, unit: '%' },
  { key: 'vignette', label: 'video.vignette', icon: <SunDim />, min: 0, max: 1, unit: '%' },
];

const LOOKS: { key: MessageKey; adjust: Partial<VideoAdjust>; swatch: string }[] = [
  { key: 'video.look.vivid', adjust: { contrast: 0.15, saturation: 0.35, brightness: 0.03 }, swatch: 'linear-gradient(135deg,#f43f5e,#f59e0b,#22c55e)' },
  { key: 'video.look.warm', adjust: { warmth: 0.55, saturation: 0.1, vignette: 0.25 }, swatch: 'linear-gradient(135deg,#fb923c,#facc15)' },
  { key: 'video.look.cool', adjust: { warmth: -0.5, contrast: 0.08 }, swatch: 'linear-gradient(135deg,#38bdf8,#6366f1)' },
  { key: 'video.look.cinema', adjust: { contrast: 0.22, saturation: -0.18, warmth: 0.15, vignette: 0.55 }, swatch: 'linear-gradient(135deg,#0f766e,#f97316)' },
  { key: 'video.look.mono', adjust: { saturation: -1, contrast: 0.2 }, swatch: 'linear-gradient(135deg,#111,#eee)' },
];

export function AdjustPanel() {
  const t = useT();
  const adjust = useEditor((s) => s.s.adjust);
  const set = useEditor((s) => s.set);
  const changed = Object.values(adjust).some((v) => Math.abs(v) > 1e-4);
  return (
    <>
      <PanelSection title={t('video.looks')}>
        <div className="no-scrollbar -mx-1 flex gap-2 overflow-x-auto px-1 pb-1">
          {LOOKS.map((l) => (
            <motion.button
              key={l.key}
              type="button"
              whileHover={{ y: -2 }}
              whileTap={{ scale: 0.94 }}
              onClick={() => set({ adjust: { ...NO_ADJUST, ...l.adjust } })}
              className="flex shrink-0 flex-col items-center gap-1.5 text-[11px] text-dim hover:text-fg"
            >
              <span className="size-12 rounded-2xl border border-line shadow-inner" style={{ background: l.swatch }} />
              {t(l.key)}
            </motion.button>
          ))}
        </div>
      </PanelSection>
      <PanelSection
        title={t('video.adjust')}
        hint={t('video.adjustHint')}
        action={
          changed && (
            <Button size="sm" variant="ghost" icon={<Undo2 className="size-4" />} onClick={() => set({ adjust: { ...NO_ADJUST } })}>
              {t('common.reset')}
            </Button>
          )
        }
      >
        {ADJUST.map((a) => (
          <div key={a.key} className="flex items-center gap-3">
            <span className={cn('grid size-8 shrink-0 place-items-center rounded-xl bg-surface-2 [&>svg]:size-4', Math.abs(adjust[a.key]) > 1e-4 ? 'text-accent' : 'text-faint')}>{a.icon}</span>
            <Slider
              className="flex-1"
              label={t(a.label)}
              value={a.unit === '°' ? adjust[a.key] : Math.round(adjust[a.key] * 100)}
              min={a.unit === '°' ? a.min : a.min * 100}
              max={a.unit === '°' ? a.max : a.max * 100}
              center={0}
              format={(v) => (a.unit === '°' ? `${v}°` : `${v > 0 ? '+' : ''}${v}`)}
              onChange={(v) => set((s) => ({ adjust: { ...s.adjust, [a.key]: a.unit === '°' ? v : v / 100 } }))}
            />
          </div>
        ))}
      </PanelSection>
    </>
  );
}

// ───────────────────────── Speed & audio ─────────────────────────

export function AudioPanel() {
  const t = useT();
  const st = useEditor((s) => s.s);
  const hasAudio = useEditor((s) => s.hasAudio);
  const set = useEditor((s) => s.set);
  return (
    <>
      <PanelSection title={t('video.speed')} hint={t('video.speedHint', { d: fmtTime(outDuration(st)) })}>
        <div className="grid grid-cols-5 gap-1.5">
          {SPEEDS.map((sp) => {
            const active = Math.abs(st.speed - sp) < 1e-6;
            return (
              <button
                key={sp}
                type="button"
                onClick={() => set({ speed: sp })}
                className={cn('relative h-9 rounded-xl text-[13px] font-semibold tabular-nums transition-colors', active ? 'text-white' : 'text-dim hover:bg-surface-2 hover:text-fg')}
                data-testid={`speed-${sp}`}
              >
                {active && <motion.span layoutId="ve-speed" className="absolute inset-0 rounded-xl bg-accent-gradient" transition={{ type: 'spring', stiffness: 500, damping: 36 }} />}
                <span className="relative">{sp}×</span>
              </button>
            );
          })}
        </div>
        <Slider
          label={t('video.customSpeed')}
          value={Math.round(st.speed * 100)}
          min={25}
          max={400}
          step={5}
          center={100}
          format={(v) => `${(v / 100).toFixed(2)}×`}
          onChange={(v) => set({ speed: v / 100 })}
        />
      </PanelSection>
      <PanelSection title={t('video.sound')} hint={hasAudio ? undefined : t('video.noAudio')}>
        {hasAudio && (
          <>
            <div className="flex items-center justify-between">
              <span className="text-sm">{t('video.muteAll')}</span>
              <Switch checked={st.mute} onChange={(mute) => set({ mute })} label={t('video.muteAll')} />
            </div>
            <Slider
              label={t('video.volume')}
              value={Math.round(st.volume * 100)}
              min={0}
              max={200}
              step={5}
              center={100}
              format={(v) => `${v}%`}
              onChange={(v) => set({ volume: v / 100, mute: false })}
            />
            {st.volume > 1 && <p className="text-xs text-faint">{t('video.volumeBoostNote')}</p>}
          </>
        )}
      </PanelSection>
    </>
  );
}

export function useDesignSize() {
  const src = useEditor((s) => s.src);
  const crop = useEditor((s) => s.s.crop);
  const rotate = useEditor((s) => s.s.rotate);
  return baseSize(src, crop, rotate);
}

