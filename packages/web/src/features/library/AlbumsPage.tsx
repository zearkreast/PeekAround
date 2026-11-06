import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { motion } from 'motion/react';
import { BookImage, Plus } from 'lucide-react';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { useChrome } from '@/store/chrome';
import { toast } from '@/store/toasts';
import { Button, EmptyState, Input, Modal, Spinner } from '@/components/ui';
import { AlbumCard } from './AlbumCard';
import { addToAlbum } from './AlbumPicker';
import { albumHref, albumsSupported, refreshAlbums, useAlbums } from './albums';
import { useEnterOnce } from './motion';
import './library.css';

/** "New album" button + name dialog (empty album; items are added from the gallery) */
export function NewAlbumButton({ variant = 'ghost', compact }: { variant?: 'ghost' | 'primary'; compact?: boolean }) {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const nav = useNavigate();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  if (!backend.createAlbum) return null;
  const create = async () => {
    setBusy(true);
    try {
      const a = await backend.createAlbum!(name.trim() || t('album.defaultName'));
      refreshAlbums(qc);
      setOpen(false);
      nav(albumHref(a.id));
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      {compact ? (
        <button
          type="button"
          title={t('album.new')}
          aria-label={t('album.new')}
          onClick={() => {
            setName('');
            setOpen(true);
          }}
          className="grid size-6 place-items-center rounded-md text-faint hover:bg-surface-2 hover:text-fg"
        >
          <Plus className="size-3.5" />
        </button>
      ) : (
        <Button
          variant={variant}
          size="sm"
          icon={<Plus className="size-4" />}
          onClick={() => {
            setName('');
            setOpen(true);
          }}
        >
          {t('album.new')}
        </Button>
      )}
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        title={t('album.new')}
        width={400}
        footer={
          <>
            <Button variant="ghost" onClick={() => setOpen(false)}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" loading={busy} onClick={() => void create()}>
              {t('common.create')}
            </Button>
          </>
        }
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void create();
          }}
        >
          <Input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder={t('album.newName')} />
        </form>
      </Modal>
    </>
  );
}

/** /albums — every album as a cover card (drop tiles on a card to add them). */
export default function AlbumsPage() {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const albums = useAlbums();
  const setChrome = useChrome((s) => s.set);
  const animate = useEnterOnce('albums-page', albums.isSuccess);
  useEffect(() => setChrome({ crumbs: [], title: t('album.title'), viewControls: false }), [setChrome, t]);
  const list = albums.data ?? [];

  if (!albumsSupported(backend)) return <EmptyState icon={<BookImage />} title={t('album.unsupported')} />;

  return (
    <div className="mx-auto w-full max-w-[1800px] px-3 pt-4 pb-28 sm:px-7 sm:pt-6">
      <motion.header
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ type: 'spring', stiffness: 260, damping: 30 }}
        className="mb-6 flex flex-wrap items-end justify-between gap-3 px-1"
      >
        <div>
          <h1 className="flex items-center gap-3 font-display text-[26px] font-bold tracking-tight sm:text-[34px]">
            <BookImage className="size-6 text-accent sm:size-7" />
            {t('album.title')}
          </h1>
          {list.length > 0 && <p className="mt-1 text-sm text-dim tabular-nums">{t('album.count', { n: list.length })}</p>}
        </div>
        <NewAlbumButton variant="primary" />
      </motion.header>
      {albums.isLoading && (
        <div className="grid h-60 place-items-center">
          <Spinner className="size-7" />
        </div>
      )}
      {albums.isSuccess && !list.length && <EmptyState icon={<BookImage />} title={t('album.empty.title')} body={t('album.empty.body')} />}
      <div className="grid grid-cols-2 gap-x-3.5 gap-y-6 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 2xl:grid-cols-7" data-testid="albums-grid">
        {list.map((a, i) => (
          <AlbumCard key={a.id} album={a} index={i} animate={animate} onDropIds={(ids) => void addToAlbum(backend, qc, a, ids, t)} />
        ))}
      </div>
    </div>
  );
}
