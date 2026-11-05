import { Smartphone } from 'lucide-react';
import { motion } from 'motion/react';
import { useBackend } from '@/backend';
import { loadMode, saveMode } from '@/backend/android/config';
import { isNative } from '@/backend/android/plugin';
import { useLang } from '@/i18n';
import { Button } from '@/components/ui';
import { L, fmt } from './strings';

/** Settings card (Android only): shows how the app is connected and lets the user switch. */
export function AndroidConnectionCard() {
  const lang = useLang();
  const s = L[lang];
  useBackend();
  if (!isNative()) return null;
  const m = loadMode();
  return (
    <motion.section initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} className="rounded-3xl border border-line bg-elev p-5 sm:p-6">
      <h2 className="mb-4 flex items-center gap-2.5 font-display text-lg font-semibold">
        <span className="grid size-8 place-items-center rounded-xl bg-surface-2 text-accent">
          <Smartphone className="size-4" />
        </span>
        {s.mode}
      </h2>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <span className="text-sm text-dim">{m?.mode === 'server' ? fmt(s.modeServer, { url: m.url }) : s.modeLocal}</span>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => {
            saveMode(null);
            location.reload();
          }}
        >
          {s.change}
        </Button>
      </div>
    </motion.section>
  );
}
