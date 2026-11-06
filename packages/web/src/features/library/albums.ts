import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query';
import { create } from 'zustand';
import type { Album, MediaItem } from '@peekaround/shared';
import { ApiError, useBackend, type Backend } from '@/backend';
import { liveSources } from '@/features/gallery/grid/source';

/** albums need server support (http backends); the Android local backend has none */
export const albumsSupported = (b: Backend) => !!(b.albums && b.addToAlbum && b.createAlbum);

export function useAlbums() {
  const b = useBackend();
  return useQuery({ queryKey: ['albums', b.id], queryFn: () => b.albums!(), enabled: albumsSupported(b), staleTime: 10_000 });
}

/** albums deleted here: their page (still mounted while it animates out) must not refetch them */
const gone = new Set<string>();

export function useAlbum(id: string | undefined) {
  const b = useBackend();
  const qc = useQueryClient();
  const key = ['albums', b.id, id];
  return useQuery({
    queryKey: key,
    queryFn: () => {
      if (!gone.has(id!)) return b.album!(id!);
      // deleted here: keep what the leaving page shows
      const last = qc.getQueryData<Album>(key);
      return last ? Promise.resolve(last) : Promise.reject(new ApiError(404, 'Album not found'));
    },
    enabled: !!id && albumsSupported(b),
    staleTime: 5_000,
    retry: (n, e) => !(e instanceof ApiError && e.status === 404) && n < 2,
  });
}

/**
 * Deleting an album here: its leaving page must not refetch it (the list refresh / server event
 * would 404). Only for a few seconds — ids can be reused by a new album. Returns "undo" (delete failed).
 */
export function forgetAlbum(id: string): () => void {
  gone.add(id);
  const t = setTimeout(() => gone.delete(id), 4000);
  return () => {
    clearTimeout(t);
    gone.delete(id);
  };
}

export const albumHref = (id: string) => `/album/${id}`;

/** after a change made here: album list + open album grids (the server event does the same for other windows) */
export function refreshAlbums(qc: QueryClient, ids?: string[]) {
  void qc.invalidateQueries({ queryKey: ['albums'] });
  for (const s of liveSources()) if (s.query.albumId && (!ids || ids.includes(s.query.albumId))) s.refresh();
}

/** the album page that is open (album actions: remove from album, make cover, move) */
export const useAlbumContext = create<{ album: Album | null; set(a: Album | null): void }>((set) => ({
  album: null,
  set: (album) => set({ album }),
}));

/** "Add to album" picker */
export const useAlbumPicker = create<{ items: MediaItem[] | null; open(items: MediaItem[]): void; close(): void }>((set) => ({
  items: null,
  open: (items) => set({ items }),
  close: () => set({ items: null }),
}));

/** albums only hold photos / videos / audio — never archives */
export const albumable = (items: MediaItem[]) => items.filter((i) => i.kind !== 'archive');
