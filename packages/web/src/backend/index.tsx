import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import type { ServerEvent, ServerInfo } from '@peekaround/shared';
import { HttpBackend } from './http';
import type { Backend, ConnectionState } from './types';
import { useJobs } from '@/store/jobs';
import { useScan } from '@/store/scan';

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
    qc.clear();
    setBackendState(b);
  };

  // live events → cache invalidation
  useEffect(() => {
    let mediaTimer: ReturnType<typeof setTimeout> | null = null;
    const pendingFolders = new Set<string>();
    const flushMedia = () => {
      mediaTimer = null;
      pendingFolders.clear();
      void qc.invalidateQueries({ queryKey: ['browse'] });
      void qc.invalidateQueries({ queryKey: ['timeline'] });
      void qc.invalidateQueries({ queryKey: ['libraries'] });
    };
    const off = backend.subscribe((e: ServerEvent) => {
      switch (e.type) {
        case 'hello':
          qc.setQueryData(['info', backend.id], e.info);
          break;
        case 'media':
          pendingFolders.add(e.folderId);
          if (!mediaTimer) mediaTimer = setTimeout(flushMedia, 900);
          break;
        case 'folders':
          void qc.invalidateQueries({ queryKey: ['libraries'] });
          void qc.invalidateQueries({ queryKey: ['browse'] });
          break;
        case 'fonts':
          void qc.invalidateQueries({ queryKey: ['fonts'] });
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
      if (mediaTimer) clearTimeout(mediaTimer);
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
