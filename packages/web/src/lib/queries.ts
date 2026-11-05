import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import type { BrowseQuery, BrowseResult, MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';

export const PAGE = 400;

export function useLibraries() {
  const b = useBackend();
  return useQuery({ queryKey: ['libraries', b.id], queryFn: () => b.libraries(), staleTime: 10_000 });
}

export function useSettings() {
  const b = useBackend();
  return useQuery({ queryKey: ['settings', b.id], queryFn: () => b.settings(), staleTime: 60_000 });
}

/** Paged browse; flattens pages and keeps the first page's folders/crumbs. */
export function useBrowse(q: BrowseQuery, enabled = true) {
  const b = useBackend();
  const query = useInfiniteQuery({
    queryKey: ['browse', b.id, q],
    enabled,
    initialPageParam: 0,
    queryFn: ({ pageParam }) => b.browse({ ...q, offset: pageParam, limit: q.limit ?? PAGE }),
    getNextPageParam: (last: BrowseResult, all: BrowseResult[]) => {
      const loaded = all.reduce((a, p) => a + p.items.length, 0);
      return loaded < last.total && last.items.length > 0 ? loaded : undefined;
    },
    staleTime: 5_000,
    placeholderData: (prev) => prev,
  });
  const first = query.data?.pages[0];
  const items: MediaItem[] = query.data?.pages.flatMap((p) => p.items) ?? [];
  return { ...query, first, items, total: first?.total ?? 0 };
}
