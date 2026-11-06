import { Link } from 'react-router';
import { Sparkles } from 'lucide-react';
import { useLang, useT } from '@/i18n';
import { Carousel, Thumb, useEnter } from './cards';
import { useExplore } from './data';
import { memoryShort, playHref } from './labels';
import { motion } from 'motion/react';
import { usePrefs } from '@/store/prefs';
import './explore.css';

/** Google-Photos-like memories row (home / timeline top). Renders nothing without memories. */
export function MemoriesStrip({ className }: { className?: string }) {
  const t = useT();
  const lang = useLang();
  const { result } = useExplore();
  const enter = useEnter();
  const list = result.memories;
  // Settings → explore: memories strip off
  const show = usePrefs((s) => s.showMemories);
  if (!list.length || !show) return null;
  return (
    <motion.section {...enter} className={className ?? 'mb-8'} data-memories-strip>
      <div className="mb-3 flex items-center gap-2">
        <Sparkles className="size-4 text-accent" />
        <h2 className="font-display text-lg font-semibold">{t('explore.memories')}</h2>
        <Link to="/explore" className="touch-target ml-auto inline-flex h-8 items-center justify-center rounded-lg px-2.5 text-[13px] font-medium text-accent hover:bg-surface-2">
          {t('explore.seeAll')}
        </Link>
      </div>
      <Carousel>
        {list.map((m) => (
          <Link key={m.id} to={playHref(m.id)} data-memory={m.id} className="group relative block aspect-[3/4] w-[30vw] max-w-[150px] shrink-0 snap-start overflow-hidden rounded-2xl bg-[var(--tile-bg)] sm:w-[150px]">
            <Thumb id={m.coverIds[0]} size={640} className="absolute inset-0 size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
            <div className="absolute inset-0 bg-gradient-to-t from-black/75 via-transparent to-transparent" />
            <div className="absolute inset-x-0 bottom-0 p-2.5 text-[13px] leading-tight font-semibold text-white [text-wrap:balance]">{memoryShort(m, lang)}</div>
          </Link>
        ))}
      </Carousel>
    </motion.section>
  );
}

export default MemoriesStrip;
