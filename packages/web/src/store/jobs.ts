import { create } from 'zustand';
import type { Job } from '@peekaround/shared';

interface JobsStore {
  jobs: Record<string, Job>;
  upsert(j: Job): void;
  setAll(js: Job[]): void;
  remove(id: string): void;
}

export const useJobs = create<JobsStore>((set) => ({
  jobs: {},
  upsert: (j) => set((s) => ({ jobs: { ...s.jobs, [j.id]: j } })),
  setAll: (js) => set({ jobs: Object.fromEntries(js.map((j) => [j.id, j])) }),
  remove: (id) =>
    set((s) => {
      const n = { ...s.jobs };
      delete n[id];
      return { jobs: n };
    }),
}));
