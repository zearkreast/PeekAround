import crypto from 'node:crypto';
import fsp from 'node:fs/promises';
import path from 'node:path';
import type { Job } from '@peekaround/shared';
import type { EventHub } from '../events.js';

export interface JobContext {
  job: Job;
  signal: AbortSignal;
  progress(p: number): void;
}

export type JobRunner = (jc: JobContext) => Promise<Job['result']>;

/**
 * Serial job queue (one ffmpeg job at a time — they already use all cores).
 * Runners are registered by the video/slideshow modules.
 */
export class JobService {
  private jobs = new Map<string, Job>();
  private queue: { job: Job; run: JobRunner }[] = [];
  private running: { job: Job; ctrl: AbortController } | null = null;
  readonly assetDir: string;

  constructor(
    private events: EventHub,
    tempDir: string,
  ) {
    this.assetDir = path.join(tempDir, 'job-assets');
  }

  list(): Job[] {
    return [...this.jobs.values()].sort((a, b) => b.createdAt - a.createdAt);
  }

  get(id: string): Job | undefined {
    return this.jobs.get(id);
  }

  enqueue(type: Job['type'], title: string, run: JobRunner): Job {
    const job: Job = {
      id: crypto.randomUUID(),
      type,
      title,
      status: 'queued',
      progress: 0,
      error: null,
      result: null,
      createdAt: Date.now(),
    };
    this.jobs.set(job.id, job);
    this.queue.push({ job, run });
    this.emit(job);
    void this.pump();
    return job;
  }

  cancel(id: string) {
    const j = this.jobs.get(id);
    if (!j) return;
    if (this.running?.job.id === id) this.running.ctrl.abort();
    else if (j.status === 'queued') {
      this.queue = this.queue.filter((q) => q.job.id !== id);
      j.status = 'canceled';
      this.emit(j);
    }
  }

  clearFinished() {
    for (const [id, j] of this.jobs) if (j.status !== 'queued' && j.status !== 'running') this.jobs.delete(id);
  }

  async saveAsset(data: AsyncIterable<Buffer> | Buffer, ext = 'png'): Promise<string> {
    await fsp.mkdir(this.assetDir, { recursive: true });
    const id = `${crypto.randomUUID()}.${ext.replace(/[^a-z0-9]/gi, '')}`;
    const file = path.join(this.assetDir, id);
    if (Buffer.isBuffer(data)) await fsp.writeFile(file, data);
    else {
      const chunks: Buffer[] = [];
      for await (const c of data) chunks.push(c);
      await fsp.writeFile(file, Buffer.concat(chunks));
    }
    return id;
  }

  assetPath(id: string): string {
    if (!/^[0-9a-f-]{36}\.[a-z0-9]+$/i.test(id)) throw new Error('Invalid asset id');
    return path.join(this.assetDir, id);
  }

  private emit(job: Job) {
    this.events.emit({ type: 'job', job: { ...job } });
  }

  private async pump() {
    if (this.running) return;
    const next = this.queue.shift();
    if (!next) return;
    const ctrl = new AbortController();
    this.running = { job: next.job, ctrl };
    const job = next.job;
    job.status = 'running';
    this.emit(job);
    let last = 0;
    try {
      job.result = await next.run({
        job,
        signal: ctrl.signal,
        progress: (p) => {
          job.progress = Math.max(0, Math.min(1, p));
          const now = Date.now();
          if (now - last > 300) {
            last = now;
            this.emit(job);
          }
        },
      });
      job.status = 'done';
      job.progress = 1;
    } catch (e: any) {
      job.status = ctrl.signal.aborted ? 'canceled' : 'error';
      job.error = ctrl.signal.aborted ? null : String(e?.message ?? e).slice(0, 500);
    } finally {
      this.running = null;
      this.emit(job);
      void this.pump();
    }
  }
}
