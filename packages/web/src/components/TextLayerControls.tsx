import { AlignCenter, AlignLeft, AlignRight, Bold, Italic } from 'lucide-react';
import type { ReactNode } from 'react';
import type { TextLayer } from '@/lib/text';
import { useFonts } from '@/lib/fonts';
import { cn } from '@/lib/cn';
import { ColorInput, Segmented, Slider, Switch } from './ui';
import { FontPicker } from './FontPicker';
import { useLang } from '@/i18n';

const L = {
  tr: {
    text: 'Metin',
    font: 'Font',
    size: 'Boyut',
    weight: 'Kalınlık',
    color: 'Renk',
    spacing: 'Harf aralığı',
    line: 'Satır yüksekliği',
    rotation: 'Döndür',
    opacity: 'Opaklık',
    stroke: 'Kontur',
    shadow: 'Gölge',
    background: 'Arka plan',
    blur: 'Bulanıklık',
    padding: 'Dolgu',
    radius: 'Köşe',
    width: 'Kalınlık',
  },
  en: {
    text: 'Text',
    font: 'Font',
    size: 'Size',
    weight: 'Weight',
    color: 'Color',
    spacing: 'Letter spacing',
    line: 'Line height',
    rotation: 'Rotate',
    opacity: 'Opacity',
    stroke: 'Outline',
    shadow: 'Shadow',
    background: 'Background',
    blur: 'Blur',
    padding: 'Padding',
    radius: 'Corners',
    width: 'Width',
  },
};

function Section({ title, on, onToggle, children }: { title: string; on: boolean; onToggle: (v: boolean) => void; children: ReactNode }) {
  return (
    <div className="rounded-2xl border border-line bg-surface p-3">
      <div className="flex items-center justify-between">
        <span className="text-[13px] font-medium text-dim">{title}</span>
        <Switch checked={on} onChange={onToggle} label={title} />
      </div>
      {on && <div className="mt-3 flex flex-col gap-3">{children}</div>}
    </div>
  );
}

/**
 * Property panel for a TextLayer — shared by the photo editor, collage and video editor.
 * `maxSize` lets callers scale the size slider to the output resolution.
 */
export function TextLayerControls({
  layer,
  onChange,
  maxSize = 400,
  className,
}: {
  layer: TextLayer;
  onChange: (patch: Partial<TextLayer>) => void;
  maxSize?: number;
  className?: string;
}) {
  const lang = useLang();
  const s = L[lang];
  const { families } = useFonts();
  const fam = families.find((f) => f.family === layer.fontFamily);
  const weights = fam?.weights ?? [400, 700];

  return (
    <div className={cn('flex flex-col gap-3', className)}>
      <textarea
        value={layer.text}
        onChange={(e) => onChange({ text: e.target.value })}
        rows={3}
        className="w-full resize-y rounded-xl border border-line bg-surface p-3 text-sm outline-none focus:border-[color-mix(in_oklab,var(--accent)_60%,transparent)]"
        style={{ fontFamily: `"${layer.fontFamily}"` }}
        placeholder={s.text}
      />
      <FontPicker
        value={layer.fontFamily}
        onChange={(family, f) => {
          const w = f.weights.includes(layer.fontWeight) ? layer.fontWeight : f.weights.reduce((a, b) => (Math.abs(b - layer.fontWeight) < Math.abs(a - layer.fontWeight) ? b : a), f.weights[0]);
          onChange({ fontFamily: family, fontWeight: w, italic: layer.italic && f.italic });
        }}
      />
      <div className="flex flex-wrap items-center gap-2">
        <Segmented
          size="sm"
          value={layer.align}
          onChange={(align) => onChange({ align })}
          options={[
            { value: 'left', label: '', icon: <AlignLeft />, title: 'Left' },
            { value: 'center', label: '', icon: <AlignCenter />, title: 'Center' },
            { value: 'right', label: '', icon: <AlignRight />, title: 'Right' },
          ]}
        />
        <button
          type="button"
          title="Bold"
          onClick={() => onChange({ fontWeight: layer.fontWeight >= 600 ? (weights.find((w) => w <= 500) ?? weights[0]) : (weights.find((w) => w >= 700) ?? weights[weights.length - 1]) })}
          className={cn('grid size-8 place-items-center rounded-lg border border-line', layer.fontWeight >= 600 ? 'bg-surface-3 text-fg' : 'text-faint')}
        >
          <Bold className="size-4" />
        </button>
        <button
          type="button"
          title="Italic"
          onClick={() => onChange({ italic: !layer.italic })}
          className={cn('grid size-8 place-items-center rounded-lg border border-line', layer.italic ? 'bg-surface-3 text-fg' : 'text-faint')}
        >
          <Italic className="size-4" />
        </button>
        <ColorInput value={layer.color} onChange={(color) => onChange({ color })} label={s.color} />
      </div>
      <Slider label={s.size} value={Math.round(layer.fontSize)} min={8} max={maxSize} onChange={(fontSize) => onChange({ fontSize })} />
      {weights.length > 1 && (
        <Slider
          label={s.weight}
          value={layer.fontWeight}
          min={weights[0]}
          max={weights[weights.length - 1]}
          step={100}
          onChange={(w) => onChange({ fontWeight: weights.reduce((a, b) => (Math.abs(b - w) < Math.abs(a - w) ? b : a), weights[0]) })}
        />
      )}
      <Slider label={s.spacing} value={layer.letterSpacing} min={-10} max={60} onChange={(letterSpacing) => onChange({ letterSpacing })} center={0} />
      <Slider label={s.line} value={layer.lineHeight} min={0.7} max={2.5} step={0.05} onChange={(lineHeight) => onChange({ lineHeight })} format={(v) => v.toFixed(2)} />
      <Slider label={s.rotation} value={layer.rotation} min={-180} max={180} onChange={(rotation) => onChange({ rotation })} format={(v) => `${v}°`} center={0} />
      <Slider label={s.opacity} value={Math.round(layer.opacity * 100)} min={0} max={100} onChange={(v) => onChange({ opacity: v / 100 })} format={(v) => `${v}%`} />

      <Section title={s.stroke} on={!!layer.stroke} onToggle={(v) => onChange({ stroke: v ? { color: '#000000', width: Math.max(2, Math.round(layer.fontSize / 24)) } : null })}>
        {layer.stroke && (
          <>
            <ColorInput value={layer.stroke.color} onChange={(color) => onChange({ stroke: { ...layer.stroke!, color } })} label={s.color} />
            <Slider label={s.width} value={layer.stroke.width} min={1} max={Math.max(20, Math.round(maxSize / 12))} onChange={(width) => onChange({ stroke: { ...layer.stroke!, width } })} />
          </>
        )}
      </Section>
      <Section title={s.shadow} on={!!layer.shadow} onToggle={(v) => onChange({ shadow: v ? { color: 'rgba(0,0,0,0.5)', blur: 16, x: 0, y: 4 } : null })}>
        {layer.shadow && (
          <>
            <ColorInput value={layer.shadow.color} onChange={(color) => onChange({ shadow: { ...layer.shadow!, color } })} label={s.color} />
            <Slider label={s.blur} value={layer.shadow.blur} min={0} max={80} onChange={(blur) => onChange({ shadow: { ...layer.shadow!, blur } })} />
            <Slider label="X" value={layer.shadow.x} min={-40} max={40} onChange={(x) => onChange({ shadow: { ...layer.shadow!, x } })} center={0} />
            <Slider label="Y" value={layer.shadow.y} min={-40} max={40} onChange={(y) => onChange({ shadow: { ...layer.shadow!, y } })} center={0} />
          </>
        )}
      </Section>
      <Section
        title={s.background}
        on={!!layer.background}
        onToggle={(v) => onChange({ background: v ? { color: 'rgba(0,0,0,0.55)', padding: Math.round(layer.fontSize * 0.3), radius: Math.round(layer.fontSize * 0.25) } : null })}
      >
        {layer.background && (
          <>
            <ColorInput value={layer.background.color} onChange={(color) => onChange({ background: { ...layer.background!, color } })} label={s.color} />
            <Slider label={s.padding} value={layer.background.padding} min={0} max={Math.max(60, Math.round(maxSize / 3))} onChange={(padding) => onChange({ background: { ...layer.background!, padding } })} />
            <Slider label={s.radius} value={layer.background.radius} min={0} max={Math.max(60, Math.round(maxSize / 3))} onChange={(radius) => onChange({ background: { ...layer.background!, radius } })} />
          </>
        )}
      </Section>
    </div>
  );
}
