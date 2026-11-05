import { motion } from 'motion/react';
import { RotateCcw } from 'lucide-react';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { Button, Slider } from '@/components/ui';
import { ADJUST_GROUPS, isNeutral } from '../adjust';
import { useEditor } from '../store';
import { NEUTRAL_ADJUST, type AdjustKey } from '../types';
import { PanelHeading, sectionMotion } from '../ToolTabs';

const signed = (v: number) => (v > 0 ? `+${v}` : `${v}`);

export function AdjustPanel() {
  const t = useT();
  const adjust = useEditor((s) => s.h.doc.adjust);
  const change = useEditor((s) => s.change);
  const set = (key: AdjustKey, v: number) => change((d) => ({ ...d, adjust: { ...d.adjust, [key]: v } }), `adj.${key}`);

  return (
    <div className="flex flex-col gap-6">
      {ADJUST_GROUPS.map((g, gi) => (
        <motion.section key={g.title} {...sectionMotion(gi)}>
          <PanelHeading>{t(g.title)}</PanelHeading>
          <div className="flex flex-col gap-2.5">
            {g.sliders.map((s) => {
              const v = adjust[s.key];
              return (
                <Slider
                  key={s.key}
                  label={
                    <span className="inline-flex items-center gap-2">
                      {s.hint && <span className="size-2.5 rounded-full ring-1 ring-white/15" style={{ background: s.hint }} />}
                      <span className={cn(v !== 0 && 'text-fg')}>{t(s.label)}</span>
                      <motion.span
                        initial={false}
                        animate={{ scale: v !== 0 ? 1 : 0, opacity: v !== 0 ? 1 : 0 }}
                        className="size-1.5 rounded-full bg-accent"
                      />
                    </span>
                  }
                  value={v}
                  min={s.min}
                  max={s.max}
                  center={0}
                  format={s.key === 'hue' ? (x) => `${signed(x)}°` : s.min < 0 ? signed : undefined}
                  onChange={(x) => set(s.key, x)}
                />
              );
            })}
          </div>
        </motion.section>
      ))}
      <motion.div {...sectionMotion(3)} className="flex items-center justify-between gap-3 border-t border-line pt-4">
        <span className="text-xs text-faint">{t('photo.resetHint')}</span>
        <Button
          size="sm"
          variant="ghost"
          icon={<RotateCcw className="size-3.5" />}
          disabled={isNeutral(adjust)}
          onClick={() => change((d) => ({ ...d, adjust: { ...NEUTRAL_ADJUST } }))}
        >
          {t('photo.resetAll')}
        </Button>
      </motion.div>
    </div>
  );
}
