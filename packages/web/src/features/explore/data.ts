import { useEffect, useMemo } from 'react';
import { useNavigate, useLocation as useRouterLocation } from 'react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import type { ExploreResult, MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { withView } from '@/lib/routes';
import { useViewer } from '@/store/viewer';
import { useWorkbench } from '@/store/workbench';
import { useSlideshow } from '@/features/slideshow/store';

const EMPTY: ExploreResult = { memories: [], places: [], types: [], cameras: [], years: [], indexing: { done: 0, total: 0 } };

export const isIndexing = (r: ExploreResult | undefined) => !!r && r.indexing.total > 0 && r.indexing.done < r.indexing.total;

/**
 * Explore groups (memories, places, types, cameras, years). Refetches on the server's throttled
 * {type:'explore'} event and polls gently while metadata indexing is still running.
 */
export function useExplore() {
  const backend = useBackend();
  const qc = useQueryClient();
  useEffect(
    () =>
      backend.subscribe((e) => {
        if (e.type === 'explore' || (e.type === 'folders' && !e.status)) void qc.invalidateQueries({ queryKey: ['explore', backend.id] });
      }),
    [backend, qc],
  );
  const q = useQuery({
    queryKey: ['explore', backend.id],
    queryFn: () => backend.explore(),
    staleTime: 30_000,
    // the server pushes {type:'explore'} (throttled) — the poll only refreshes the indexing progress;
    // Android local mode has no push events for it
    refetchInterval: (query) => (isIndexing(query.state.data) ? (backend.kind === 'http' ? 20_000 : 6000) : false),
  });
  return { ...q, result: q.data ?? EMPTY };
}

/** open the shared viewer on `item` (the page must have published its source via useLocation.setView) */
export function useOpenViewer() {
  const nav = useNavigate();
  const routerLoc = useRouterLocation();
  return useMemo(
    () => (item: Pick<MediaItem, 'id'>) => {
      useViewer.getState().setActive(item.id);
      requestAnimationFrame(() => nav({ search: withView(routerLoc.search, item.id) }, { state: { viewer: true } }));
    },
    [nav, routerLoc.search],
  );
}

/** start the slideshow player right away with these items (exit lands in the slideshow builder) */
export function usePlaySlideshow() {
  const nav = useNavigate();
  return useMemo(
    () => (items: MediaItem[]) => {
      const list = items.filter((m) => m.kind === 'image' || m.kind === 'video');
      if (!list.length) return;
      useWorkbench.getState().set(list, null);
      useSlideshow.getState().sync(list, null);
      nav('/slideshow/play');
    },
    [nav],
  );
}
