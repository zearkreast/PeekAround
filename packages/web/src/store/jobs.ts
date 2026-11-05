import { create } from 'zustand';
import type { Job } from '@peekaround/shared';

/** Jobs that live on the phone (native transfers) use this id prefix; the server never sees them. */
export const NATIVE_JOB = 'native:';
export const isNativeJob = (id: string) => id.startsWith(NATIVE_JOB);

interface Sample {
  at: number;
  bytes: number;
  /** bytes per second (smoothed) */
  rate: number;
}

interface JobsStore {
  jobs: Record<string, Job>;
  /** transfer speed samples by job id (bytes/s) */
  rates: Record<string, number>;
  upsert(j: Job): void;
  /** replace the server's jobs (native phone transfers are kept) */
  setAll(js: Job[]): void;
  remove(id: string): void;
  /** drop finished native transfers (the server clears its own) */
  clearNative(): void;
}

const samples = new Map<string, Sample>();

function sample(j: Job): number | undefined {
  if (j.status !== 'running' || !j.bytes) {
    samples.delete(j.id);
    return undefined;
  }
  const now = Date.now();
  const prev = samples.get(j.id);
  if (!prev) {
    samples.set(j.id, { at: now, bytes: j.bytes.done, rate: 0 });
    return undefined;
  }
  const dt = (now - prev.at) / 1000;
  if (dt < 0.4) return prev.rate || undefined;
  const inst = Math.max(0, (j.bytes.done - prev.bytes) / dt);
  const rate = prev.rate ? prev.rate * 0.6 + inst * 0.4 : inst;
  samples.set(j.id, { at: now, bytes: j.bytes.done, rate });
  return rate;
}

export const useJobs = create<JobsStore>((set) => ({
  jobs: {},
  rates: {},
  upsert: (j) =>
    set((s) => {
      const r = sample(j);
      const rates = { ...s.rates };
      if (r === undefined) delete rates[j.id];
      else rates[j.id] = r;
      return { jobs: { ...s.jobs, [j.id]: j }, rates };
    }),
  setAll: (js) =>
    set((s) => ({
      jobs: { ...Object.fromEntries(Object.entries(s.jobs).filter(([id]) => isNativeJob(id))), ...Object.fromEntries(js.map((j) => [j.id, j])) },
    })),
  remove: (id) =>
    set((s) => {
      const n = { ...s.jobs };
      delete n[id];
      return { jobs: n };
    }),
  clearNative: () =>
    set((s) => ({ jobs: Object.fromEntries(Object.entries(s.jobs).filter(([id, j]) => !isNativeJob(id) || j.status === 'running' || j.status === 'queued')) })),
}));

/** seconds left at the current speed, or null */
export function etaOf(j: Job, rate: number | undefined): number | null {
  if (!j.bytes || !rate || rate < 1 || j.status !== 'running') return null;
  const left = j.bytes.total - j.bytes.done;
  return left > 0 ? left / rate : null;
}
