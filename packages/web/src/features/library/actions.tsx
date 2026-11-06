/**
 * Library + album media actions (registered into the gallery action registry):
 *  - "Albüme ekle"        any photo / video / audio (local, phone, MEGA) — album picker
 *  - "Albümden çıkar"     on an album page
 *  - "Albüm kapağı yap"   on an album page (one item)
 *  - "Başa / sona taşı"   on an album page (switches it to the custom order)
 *  - "Kapak yap"          on library / folder pages: the item becomes its library's cover
 */
import { ArrowDownToLine, ArrowUpToLine, BookImage, ImageMinus, ImageUp, Star } from 'lucide-react';
import type { Library, MediaItem } from '@peekaround/shared';
import { registerMediaAction, type ActionCtx } from '@/features/gallery/actions';
import { queryClient } from '@/lib/queryClient';
import { albumable, albumsSupported, refreshAlbums, useAlbumContext, useAlbumPicker } from './albums';

const media = (i: MediaItem) => i.kind === 'image' || i.kind === 'video';
const currentAlbum = () => useAlbumContext.getState().album;

/** the library a media item belongs to (from the libraries cache) */
export function libraryOf(c: Pick<ActionCtx, 'backend'>, folderId: string): Library | null {
  const libs = queryClient.getQueryData<{ libraries: Library[] }>(['libraries', c.backend.id]);
  return libs?.libraries.find((l) => l.folders.some((f) => f.id === folderId)) ?? null;
}

const onLibraryPage = () => typeof location !== 'undefined' && /^\/(library|folder)\//.test(location.pathname);

registerMediaAction({
  id: 'add-to-album',
  icon: <BookImage />,
  labelKey: 'album.addTo',
  shortLabelKey: 'album.addToShort',
  group: 'organize',
  order: 5,
  clearsSelection: false,
  visible: (items, c) => albumsSupported(c.backend) && albumable(items).length > 0,
  run: (items) => useAlbumPicker.getState().open(items),
});

registerMediaAction({
  id: 'album-remove',
  icon: <ImageMinus />,
  labelKey: 'album.remove',
  group: 'organize',
  order: 6,
  clearsSelection: true,
  visible: (items, c) => !!currentAlbum() && !!c.backend.removeFromAlbum && items.length > 0,
  run: async (items, c) => {
    const a = currentAlbum();
    if (!a || !c.backend.removeFromAlbum) return;
    const r = await c.backend.removeFromAlbum(a.id, items.map((i) => i.id));
    refreshAlbums(c.qc, [a.id]);
    c.toast.success(c.t('album.removed', { n: r.removed }));
  },
});

registerMediaAction({
  id: 'album-cover',
  icon: <Star />,
  labelKey: 'album.setCover',
  group: 'organize',
  order: 7,
  surfaces: ['menu', 'viewer'],
  visible: (items, c) => !!currentAlbum() && !!c.backend.updateAlbum && items.length === 1 && media(items[0]),
  run: async (items, c) => {
    const a = currentAlbum();
    if (!a || !c.backend.updateAlbum) return;
    await c.backend.updateAlbum(a.id, { coverId: items[0].id });
    refreshAlbums(c.qc, [a.id]);
    c.toast.success(c.t('album.coverSet'));
  },
});

async function moveInAlbum(items: MediaItem[], c: ActionCtx, first: boolean) {
  const a = currentAlbum();
  if (!a || !c.backend.reorderAlbum || !c.backend.updateAlbum) return;
  let before: string | null = null;
  if (first) {
    // the current first item in the custom order
    const r = await c.backend.browse({ albumId: a.id, byPosition: true, limit: 1, kind: 'all' });
    before = r.items[0]?.id ?? null;
  }
  await c.backend.reorderAlbum(a.id, items.map((i) => i.id), before);
  refreshAlbums(c.qc, [a.id]);
}

registerMediaAction({
  id: 'album-move-first',
  icon: <ArrowUpToLine />,
  labelKey: 'album.moveFirst',
  group: 'organize',
  order: 8,
  surfaces: ['menu', 'selection'],
  visible: (items, c) => !!currentAlbum() && !!c.backend.reorderAlbum && items.length > 0,
  run: (items, c) => moveInAlbum(items, c, true),
});

registerMediaAction({
  id: 'album-move-last',
  icon: <ArrowDownToLine />,
  labelKey: 'album.moveLast',
  group: 'organize',
  order: 9,
  surfaces: ['menu', 'selection'],
  visible: (items, c) => !!currentAlbum() && !!c.backend.reorderAlbum && items.length > 0,
  run: (items, c) => moveInAlbum(items, c, false),
});

registerMediaAction({
  id: 'library-cover',
  icon: <ImageUp />,
  labelKey: 'lib2.setCover',
  group: 'organize',
  order: 40,
  surfaces: ['menu', 'viewer'],
  visible: (items, c) => c.writable && items.length === 1 && media(items[0]) && !currentAlbum() && onLibraryPage() && !!libraryOf(c, items[0].folderId),
  run: async (items, c) => {
    const lib = libraryOf(c, items[0].folderId);
    if (!lib) return;
    await c.backend.updateLibrary(lib.id, { coverId: items[0].id });
    await c.qc.invalidateQueries({ queryKey: ['libraries'] });
    c.toast.success(c.t('lib2.coverSet'));
  },
});
