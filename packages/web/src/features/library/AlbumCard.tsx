import { useState, type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router';
import { motion } from 'motion/react';
import { Images } from 'lucide-react';
import type { Album } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatMonthKey } from '@/lib/format';
import { albumHref } from './albums';
import { isMediaDrag, readMediaDrop } from './dnd';
import { stagger } from './motion';
import { heroNavigate } from './vt';

/** "Mar 2023 – Jun 2024" (or one month) from a localAt range */
export function rangeLabel(from: number | null, to: number | null, lang: 'tr' | 'en'): string | null {
  if (from == null || to == null) return null;
  const key = (ms: number) => new Date(ms).toISOString().slice(0, 7);
  const a = formatMonthKey(key(from), lang);
  const b = formatMonthKey(key(to), lang);
  return a === b ? a : `${a} – ${b}`;
}

/** Album cover tile (Albums page, Library hub). Drop media on it to add them. */
export function AlbumCard({
  album,
  index,
  animate,
  onDropIds,
  menu,
}: {
  album: Album;
  index: number;
  animate: boolean;
  onDropIds?: (ids: string[]) => void;
  menu?: ReactNode;
}) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const nav = useNavigate();
  const [over, setOver] = useState(false);
  const range = rangeLabel(album.from, album.to, lang);
  return (
    <motion.div
      initial={animate ? { opacity: 0, y: 14, scale: 0.98 } : false}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={{ type: 'spring', stiffness: 320, damping: 30, delay: animate ? stagger(index) : 0 }}
      className="group relative"
      onDragOver={(e) => {
        if (!onDropIds || !isMediaDrag(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'copy';
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        if (!onDropIds || !isMediaDrag(e)) return;
        e.preventDefault();
        setOver(false);
        onDropIds(readMediaDrop(e));
      }}
    >
      <Link
        to={albumHref(album.id)}
        onClick={(e) => heroNavigate(e, nav, albumHref(album.id), `album-${album.id}`)}
        className="block rounded-2xl outline-offset-4"
        data-album-card={album.id}
      >
        <div
          data-hero
          className={cn(
            'card-lift relative aspect-square overflow-hidden rounded-2xl border border-line bg-surface',
            over && 'ring-2 ring-[var(--accent)] ring-offset-2 ring-offset-[var(--bg)]',
          )}
        >
          {album.coverId ? (
            <img
              src={backend.thumbUrl(album.coverId, 640)}
              alt=""
              draggable={false}
              loading="lazy"
              decoding="async"
              onLoad={(e) => (e.currentTarget.dataset.loaded = '1')}
              className="fade-img size-full object-cover transition-transform duration-700 ease-[var(--ease-out)] group-hover:scale-[1.04]"
            />
          ) : (
            <div className="grid size-full place-items-center bg-gradient-to-br from-[color-mix(in_oklab,var(--accent)_22%,transparent)] to-transparent">
              <Images className="size-10 text-faint" />
            </div>
          )}
        </div>
        <div className="mt-2.5 px-0.5">
          <div className="truncate text-[14px] font-semibold">{album.name}</div>
          <div className="truncate text-xs text-dim tabular-nums">
            {t('album.items', { n: album.count })}
            {range ? ` · ${range}` : ''}
          </div>
        </div>
      </Link>
      {menu}
    </motion.div>
  );
}
