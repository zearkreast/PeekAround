import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import type { ServerEvent, ServerInfo } from '@peekaround/shared';
import { HttpBackend } from './http';
import type { Backend, ConnectionState } from './types';
import { useJobs } from '@/store/jobs';
import { useScan } from '@/store/scan';
import { dropSources, liveSources } from '@/features/gallery/grid/source';

export * from './types';
export { HttpBackend } from './http';

interface BackendCtx {
  backend: Backend;
  setBackend(b: Backend): void;
}

const Ctx = createContext<BackendCtx | null>(null);

export function BackendProvider({ initial, children }: { initial: Backend; children: ReactNode }) {
  const [backend, setBackendState] = useState<Backend>(initial);
  const prev = useRef(backend);
  const qc = useQueryClient();

  const setBackend = (b: Backend) => {
    if (prev.current !== b) prev.current.dispose();
    prev.current = b;
    dropSources();
    qc.clear();
    setBackendState(b);
  };

  // live events → targeted cache updates (no blanket refetch storms):
  //   thumb        → re-request the loaded pages in place (same layout)
  //   meta         → refetch the skeleton (dates / sizes may move items), throttled
  //   add/remove/… → refetch the skeleton + folder lists, coalesced
  useEffect(() => {
    const timers = new Map<string, ReturnType<typeof setTimeout>>();
    const pending = { thumb: new Set<string>(), meta: new Set<string>(), struct: new Set<string>() };
    const forFolders = (set: Set<string>, fn: (s: ReturnType<typeof liveSources>[number]) => void) => {
      const all = set.has('*');
      for (const src of liveSources()) if (all || [...set].some((f) => src.matches(f))) fn(src);
      set.clear();
    };
    const later = (key: string, ms: number, fn: () => void) => {
      if (timers.has(key)) return;
      timers.set(
        key,
        setTimeout(() => {
          timers.delete(key);
          fn();
        }, ms),
      );
    };
    const nonSkeleton = { queryKey: ['browse'], predicate: (q: { queryKey: readonly unknown[] }) => q.queryKey[2] !== 'skeleton' };
    const off = backend.subscribe((e: ServerEvent) => {
      switch (e.type) {
        case 'hello':
          qc.setQueryData(['info', backend.id], e.info);
          break;
        case 'media': {
          const scanning = useScan.getState().scanning;
          if (e.reason === 'thumb') {
            pending.thumb.add(e.folderId);
            later('thumb', 3000, () => forFolders(pending.thumb, (s) => s.refreshPages()));
          } else if (e.reason === 'meta') {
            pending.meta.add(e.folderId);
            later('meta', scanning ? 5000 : 2500, () => {
              forFolders(pending.meta, (s) => s.refresh());
              void qc.invalidateQueries({ queryKey: ['timeline'] });
            });
          } else {
            pending.struct.add(e.folderId);
            later('struct', scanning ? 2000 : 700, () => {
              forFolders(pending.struct, (s) => s.refresh());
              void qc.invalidateQueries(nonSkeleton);
              void qc.invalidateQueries({ queryKey: ['timeline'] });
              later('libraries', 3000, () => {
                void qc.invalidateQueries({ queryKey: ['libraries'] });
                // deleted / moved media leave albums (counts, covers)
                void qc.invalidateQueries({ queryKey: ['albums'] });
              });
            });
          }
          break;
        }
        case 'folders':
          void qc.invalidateQueries({ queryKey: ['libraries'] });
          void qc.invalidateQueries({ queryKey: ['browse'] });
          break;
        case 'fonts':
          void qc.invalidateQueries({ queryKey: ['fonts'] });
          break;
        case 'albums':
          // album list / covers / counts + open album grids
          void qc.invalidateQueries({ queryKey: ['albums'] });
          for (const src of liveSources()) if (src.query.albumId && (!e.ids || e.ids.includes(src.query.albumId))) src.refresh();
          break;
        case 'scan':
          useScan.getState().set(e);
          break;
        case 'job':
          useJobs.getState().upsert(e.job);
          if (e.job.status === 'done') {
            void qc.invalidateQueries({ queryKey: ['browse'] });
          }
          break;
      }
    });
    return () => {
      off();
      for (const t of timers.values()) clearTimeout(t);
    };
  }, [backend, qc]);

  const value = useMemo(() => ({ backend, setBackend }), [backend]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useBackendCtx(): BackendCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error('BackendProvider missing');
  return c;
}

export function useBackend(): Backend {
  return useBackendCtx().backend;
}

export function useInfo(): ServerInfo | undefined {
  const b = useBackend();
  return useQuery({ queryKey: ['info', b.id], queryFn: () => b.info(), staleTime: 30_000 }).data;
}

export function useConnection(): ConnectionState {
  const b = useBackend();
  const [s, setS] = useState<ConnectionState>('connecting');
  useEffect(() => b.onConnection(setS), [b]);
  return s;
}
