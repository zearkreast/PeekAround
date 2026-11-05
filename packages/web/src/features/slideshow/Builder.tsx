import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router';
import { motion } from 'motion/react';
import { CalendarDays, Images, Music, Play, Plus, Presentation } from 'lucide-react';
import { useT } from '@/i18n';
import { formatDuration } from '@/lib/format';
import { useChrome } from '@/store/chrome';
import { useWorkbench } from '@/store/workbench';
import { Button, EmptyState } from '@/components/ui';
import { ItemList } from './ItemList';
import { MediaPicker } from './MediaPicker';
import { ExportCard, SettingsPanel } from './SettingsPanel';
import { ASPECT_VALUES, playlist, showLength } from './show';
import { ShowStage } from './ShowStage';
import { useSlideshow } from './store';
import { useShow } from './useShow';

/** Live, muted, looping preview of the first slides with the chosen style. */
function PreviewHero({ onPlay }: { onPlay: () => void }) {
  const t = useT();
  const items = useSlideshow((s) => s.items);
  const settings = useSlideshow((s) => s.settings);
  const slides = useMemo(() => playlist(items.slice(0, 12), { ...settings, shuffle: false }), [items, settings]);
  const td = settings.transition === 'none' ? 0 : settings.transitionDuration;
  const show = useShow(slides, td, { autoplay: true, loop: true });
  const full = useMemo(() => playlist(items, { ...settings, shuffle: false }, () => 0), [items, settings]);
  const total = showLength(full, td);
  const photos = items.filter((i) => i.media.kind === 'image').length;
  const videos = items.length - photos;
  return (
    <motion.div
      initial={{ opacity: 0, y: 16, scale: 0.985 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={{ type: 'spring', stiffness: 220, damping: 26 }}
      className="group relative overflow-hidden rounded-3xl border border-line bg-black"
      data-testid="slideshow-preview"
    >
      <div className="pointer-events-none absolute inset-0 opacity-60">
        <div className="aurora" />
      </div>
      <div className="relative flex h-[clamp(240px,46vh,540px)] items-center justify-center p-4 sm:p-6">
        <div className="relative h-full max-w-full overflow-hidden rounded-2xl shadow-[0_30px_80px_-20px_rgb(0_0_0/0.8)]" style={{ aspectRatio: String(ASPECT_VALUES[settings.aspect]) }}>
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
            size={640}
            className="size-full"
          />
        </div>
      </div>
      <div className="pointer-events-none absolute inset-x-0 bottom-0 h-32 bg-gradient-to-t from-black/80 to-transparent" />
      <div className="absolute inset-x-0 bottom-0 flex flex-wrap items-end justify-between gap-3 p-5">
        <div className="text-white">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-white/75 tabular-nums">
            <span className="inline-flex items-center gap-1.5">
              <Images className="size-4" />
              {t('home.stats', { photos, videos })}
            </span>
            <span className="inline-flex items-center gap-1.5">
              <CalendarDays className="size-4" />
              {formatDuration(total)}
            </span>
            {settings.music && (
              <span className="inline-flex max-w-[40vw] items-center gap-1.5 truncate">
                <Music className="size-4 shrink-0" />
                <span className="truncate">{settings.music.name}</span>
              </span>
            )}
          </div>
        </div>
        <motion.button
          type="button"
          onClick={onPlay}
          whileHover={{ scale: 1.04 }}
          whileTap={{ scale: 0.95 }}
          className="inline-flex h-12 items-center gap-2.5 rounded-2xl bg-white px-6 text-[15px] font-semibold text-black shadow-[0_10px_40px_-10px_rgb(255_255_255/0.6)]"
          data-testid="play-show"
        >
          <Play className="size-5 fill-current" />
          {t('slide.play')}
        </motion.button>
      </div>
    </motion.div>
  );
}

export function Builder() {
  const t = useT();
  const nav = useNavigate();
  const wbItems = useWorkbench((s) => s.items);
  const wbTarget = useWorkbench((s) => s.target);
  const sync = useSlideshow((s) => s.sync);
  const items = useSlideshow((s) => s.items);
  const add = useSlideshow((s) => s.add);
  const setSettings = useSlideshow((s) => s.setSettings);
  const setChrome = useChrome((s) => s.set);
  const [picker, setPicker] = useState<'visual' | 'audio' | null>(null);

  useEffect(() => sync(wbItems, wbTarget), [wbItems, wbTarget, sync]);
  useEffect(() => setChrome({ crumbs: [], title: t('nav.slideshow'), viewControls: false }), [setChrome, t]);

  const play = () => nav('/slideshow/play');

  return (
    <div className="mx-auto w-full max-w-[1500px] px-4 pt-6 pb-28 sm:px-7" data-testid="slideshow-builder">
      <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ type: 'spring', stiffness: 260, damping: 30 }} className="mb-7 flex flex-wrap items-end gap-x-6 gap-y-3">
        <div className="min-w-0 flex-1">
          <h1 className="flex items-center gap-3 font-display text-3xl font-bold tracking-tight sm:text-[34px]">
            <Presentation className="size-7 text-accent" />
            <span className="text-gradient">{t('nav.slideshow')}</span>
          </h1>
          <p className="mt-1.5 text-sm text-dim">{t('slide.subtitle')}</p>
        </div>
        {items.length > 0 && (
          <div className="flex items-center gap-2">
            <Button variant="secondary" icon={<Plus className="size-4" />} onClick={() => setPicker('visual')}>
              {t('slide.addMedia')}
            </Button>
            <Button variant="primary" icon={<Play className="size-4 fill-current" />} onClick={play}>
              {t('slide.play')}
            </Button>
          </div>
        )}
      </motion.div>

      {items.length === 0 ? (
        <EmptyState
          icon={<Presentation />}
          title={t('slide.emptyTitle')}
          body={t('slide.emptyBody')}
          action={
            <div className="flex flex-wrap justify-center gap-2">
              <Button variant="secondary" icon={<Plus className="size-4" />} onClick={() => setPicker('visual')} data-testid="empty-add">
                {t('slide.addMedia')}
              </Button>
              <Button variant="primary" onClick={() => nav('/timeline')}>
                {t('slide.goTimeline')}
              </Button>
            </div>
          }
        />
      ) : (
        <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_380px]">
          <div className="flex min-w-0 flex-col gap-6">
            <PreviewHero onPlay={play} />
            <ItemList onAdd={() => setPicker('visual')} />
          </div>
          <div className="flex flex-col gap-4">
            <SettingsPanel onPickMusic={() => setPicker('audio')} />
            <ExportCard />
          </div>
        </div>
      )}

      <MediaPicker
        open={picker !== null}
        mode={picker ?? 'visual'}
        onClose={() => setPicker(null)}
        onPick={(m) => {
          if (picker === 'audio') setSettings({ music: m[0] ?? null });
          else add(m);
        }}
      />
    </div>
  );
}
