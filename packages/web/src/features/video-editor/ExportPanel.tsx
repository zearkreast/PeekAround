import type { ReactNode } from 'react';
import { motion } from 'motion/react';
import { Download, Film, Globe, ImagePlay } from 'lucide-react';
import type { MediaDetails, VideoOverlay } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { ensureFont } from '@/lib/fonts';
import { useSettings } from '@/lib/queries';
import { drawTextLayer, type TextLayer } from '@/lib/text';
import { Button, Segmented } from '@/components/ui';
import { useJobRun } from './JobProgress';
import { SHORT_SIDES, baseName, baseSize, buildSpec, exportSize, outDuration, overlayGroups, type Format, type Quality, type Size } from './model';
import { PanelSection } from './panels';
import { useEditor } from './store';
import { fmtTime } from './Timeline';

/** Render a group of text layers to a transparent PNG at the exact output size. */
export async function renderOverlay(layers: TextLayer[], out: Size, scale: number): Promise<Blob> {
  await Promise.all(layers.map((l) => ensureFont(l.fontFamily, l.fontWeight, l.italic)));
  const canvas = document.createElement('canvas');
  canvas.width = out.w;
  canvas.height = out.h;
  const ctx = canvas.getContext('2d')!;
  for (const l of layers) drawTextLayer(ctx, l, scale);
  return new Promise((res, rej) => canvas.toBlob((b) => (b ? res(b) : rej(new Error('PNG encoding failed'))), 'image/png'));
}

/** Start an export of the current edit (overlays → upload → job). */
export function useVideoExport(item: MediaDetails) {
  const backend = useBackend();
  const run = useJobRun('video');
  const start = () =>
    run.start(async (onPrep) => {
      const { s, src, duration } = useEditor.getState();
      const base = baseSize(src, s.crop, s.rotate);
      const out = exportSize(src, s);
      const groups = overlayGroups(s.texts, outDuration(s));
      const overlays: VideoOverlay[] = [];
      for (let i = 0; i < groups.length; i++) {
        const g = groups[i];
        const png = await renderOverlay(g.layers, out, out.h / base.h);
        overlays.push({ assetId: await backend.uploadJobAsset(png, 'png'), start: g.start, end: g.end });
        onPrep((i + 1) / groups.length);
      }
      return backend.startVideoEdit(buildSpec(item.id, s, src, duration, overlays));
    });
  return { run, start };
}

const FORMATS: { value: Format; icon: ReactNode; title: string; hint: 'video.mp4Hint' | 'video.webmHint' | 'video.gifHint' }[] = [
  { value: 'mp4', icon: <Film />, title: 'MP4', hint: 'video.mp4Hint' },
  { value: 'webm', icon: <Globe />, title: 'WebM', hint: 'video.webmHint' },
  { value: 'gif', icon: <ImagePlay />, title: 'GIF', hint: 'video.gifHint' },
];

export function ExportPanel({ item, onExport, busy, disabled }: { item: MediaDetails; onExport: () => void; busy: boolean; disabled: string | null }) {
  const t = useT();
  const src = useEditor((s) => s.src);
  const st = useEditor((s) => s.s);
  const hasAudio = useEditor((s) => s.hasAudio);
  const suffix = useSettings().data?.editSuffix ?? '_edit';
  const set = useEditor((s) => s.set);
  const base = baseSize(src, st.crop, st.rotate);
  const short = Math.min(base.w, base.h);
  const out = exportSize(src, st);
  const output = (p: Partial<typeof st.output>) => set({ output: { ...st.output, ...p } });
  const audio = hasAudio && !st.mute && st.output.format !== 'gif';

  return (
    <>
      <PanelSection title={t('video.format')}>
        <div className="grid grid-cols-3 gap-2">
          {FORMATS.map((f) => {
            const active = st.output.format === f.value;
            return (
              <motion.button
                key={f.value}
                type="button"
                whileTap={{ scale: 0.95 }}
                onClick={() => output({ format: f.value })}
                className={cn(
                  'relative flex flex-col items-start gap-1 overflow-hidden rounded-2xl border p-3 text-left transition-colors',
                  active ? 'border-transparent text-fg' : 'border-line text-dim hover:bg-surface-2',
                )}
                data-testid={`format-${f.value}`}
              >
                {active && <motion.span layoutId="ve-format" className="absolute inset-0 rounded-2xl border-2 border-[var(--accent)] bg-[color-mix(in_oklab,var(--accent)_12%,transparent)]" transition={{ type: 'spring', stiffness: 500, damping: 36 }} />}
                <span className={cn('relative [&>svg]:size-5', active && 'text-accent')}>{f.icon}</span>
                <span className="relative text-sm font-semibold">{f.title}</span>
                <span className="relative text-[11px] leading-tight text-faint">{t(f.hint)}</span>
              </motion.button>
            );
          })}
        </div>
      </PanelSection>
      <PanelSection title={t('video.resolution')}>
        <Segmented
          className="w-full [&>button]:flex-1"
          value={String(st.output.short ?? 0)}
          onChange={(v) => output({ short: v === '0' ? null : Number(v) })}
          options={SHORT_SIDES.filter((s) => s === null || s < short).map((s) => ({ value: String(s ?? 0), label: s === null ? t('video.original') : `${s}p` }))}
        />
        <Segmented
          className="w-full [&>button]:flex-1"
          value={st.output.quality}
          onChange={(quality: Quality) => output({ quality })}
          options={[
            { value: 'high', label: t('video.qHigh') },
            { value: 'medium', label: t('video.qMedium') },
            { value: 'low', label: t('video.qLow') },
          ]}
        />
      </PanelSection>
      <PanelSection title={t('video.summary')}>
        <dl className="grid grid-cols-2 gap-x-3 gap-y-2 rounded-2xl bg-surface p-3 text-[13px]">
          <dt className="text-faint">{t('video.outputSize')}</dt>
          <dd className="text-right font-mono tabular-nums">
            {out.w} × {out.h}
          </dd>
          <dt className="text-faint">{t('viewer.duration')}</dt>
          <dd className="text-right font-mono tabular-nums">{fmtTime(outDuration(st))}</dd>
          <dt className="text-faint">{t('video.sound')}</dt>
          <dd className="text-right">{audio ? t('common.yes') : t('common.no')}</dd>
          <dt className="text-faint">{t('video.texts')}</dt>
          <dd className="text-right tabular-nums">{st.texts.length}</dd>
          <dt className="text-faint">{t('video.fileName')}</dt>
          <dd className="truncate text-right" title={`${baseName(item.name)}${suffix}.${st.output.format}`}>
            {baseName(item.name)}
            {suffix}.{st.output.format}
          </dd>
        </dl>
        {st.output.format === 'gif' && <p className="text-xs text-faint">{t('video.gifNote')}</p>}
        <Button variant="primary" size="lg" icon={<Download className="size-5" />} onClick={onExport} loading={busy} disabled={!!disabled} title={disabled ?? undefined} data-testid="export-start">
          {t('video.export')}
        </Button>
        <p className="text-center text-xs text-faint">{disabled ?? t('tools.copyNote')}</p>
      </PanelSection>
    </>
  );
}
