import { useEffect, useMemo, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { Check, Images, Minus, Plus } from 'lucide-react';
import type { Album, MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { go } from '@/lib/nav';
import { toast } from '@/store/toasts';
import { Button, Input, Modal, Spinner } from '@/components/ui';
import { albumHref, albumable, refreshAlbums, useAlbumPicker, useAlbums } from './albums';

/** add media to an album (+ toast with "open"); shared by the picker and sidebar / card drops */
export async function addToAlbum(backend: ReturnType<typeof useBackend>, qc: ReturnType<typeof useQueryClient>, album: Pick<Album, 'id' | 'name'>, ids: string[], t: ReturnType<typeof useT>) {
  if (!backend.addToAlbum || !ids.length) return;
  try {
    const r = await backend.addToAlbum(album.id, ids);
    refreshAlbums(qc, [album.id]);
    if (r.added) toast.success(t('album.added', { n: r.added, name: album.name }), undefined, { label: t('common.open'), run: () => go(albumHref(album.id)) });
    else toast.info(t('album.alreadyIn', { name: album.name }));
  } catch (e) {
    toast.error(t('common.error'), (e as Error).message);
  }
}

/** "Add to album": pick an album (check marks = already inside) or create one with the items. */
export function AlbumPicker() {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const raw = useAlbumPicker((s) => s.items);
  const close = useAlbumPicker((s) => s.close);
  const items = useMemo(() => (raw ? albumable(raw) : null), [raw]);
  const albums = useAlbums();
  const [name, setName] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [counts, setCounts] = useState<Record<string, number>>({});

  useEffect(() => {
    setName('');
    // no check marks of the previous items while this selection's membership loads
    setCounts({});
    if (!items?.length || !backend.albumMembership) return;
    let alive = true;
    backend
      .albumMembership(items.map((i) => i.id))
      .then((c) => alive && setCounts(c))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [items, backend]);

  const n = items?.length ?? 0;
  const pick = async (a: Album) => {
    if (!items) return;
    setBusy(a.id);
    await addToAlbum(backend, qc, a, items.map((i) => i.id), t);
    setBusy(null);
    close();
  };
  const create = async () => {
    if (!items || !backend.createAlbum) return;
    setBusy('new');
    try {
      const a = await backend.createAlbum(name.trim() || t('album.defaultName'), items.map((i) => i.id));
      refreshAlbums(qc);
      toast.success(t('album.added', { n: a.count, name: a.name }), undefined, { label: t('common.open'), run: () => go(albumHref(a.id)) });
      close();
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal open={!!items} onClose={close} title={t('album.pickTitle', { n })} width={460}>
      <form
        className="mb-3 flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void create();
        }}
      >
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={t('album.newName')} className="flex-1" icon={<Plus />} />
        <Button type="submit" variant="primary" loading={busy === 'new'}>
          {t('album.create')}
        </Button>
      </form>
      <div className="-mx-2 max-h-[min(52vh,420px)] overflow-y-auto px-2 pb-1" data-testid="album-picker-list">
        {albums.isLoading && (
          <div className="grid h-24 place-items-center">
            <Spinner className="size-5" />
          </div>
        )}
        {albums.data?.map((a) => {
          const inside = counts[a.id] ?? 0;
          const all = n > 0 && inside >= n;
          return (
            <button
              key={a.id}
              type="button"
              disabled={!!busy}
              onClick={() => void pick(a)}
              className="flex w-full items-center gap-3 rounded-xl px-2 py-2 text-left transition-colors hover:bg-surface-2 disabled:opacity-60"
            >
              <span className="size-12 shrink-0 overflow-hidden rounded-xl bg-surface">
                {a.coverId ? (
                  <img src={backend.thumbUrl(a.coverId, 320)} alt="" className="size-full object-cover" draggable={false} />
                ) : (
                  <span className="grid size-full place-items-center text-faint">
                    <Images className="size-5" />
                  </span>
                )}
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium">{a.name}</span>
                <span className="block text-xs text-dim tabular-nums">{t('album.items', { n: a.count })}</span>
              </span>
              {busy === a.id ? (
                <Spinner className="size-4" />
              ) : inside > 0 ? (
                <span
                  title={t('album.contains')}
                  className={cn('grid size-6 place-items-center rounded-full', all ? 'bg-accent text-white' : 'bg-surface-3 text-dim')}
                >
                  {all ? <Check className="size-3.5" strokeWidth={3} /> : <Minus className="size-3.5" strokeWidth={3} />}
                </span>
              ) : null}
            </button>
          );
        })}
      </div>
    </Modal>
  );
}

export type { MediaItem };
