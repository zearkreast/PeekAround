import crypto from 'node:crypto';
import fsp from 'node:fs/promises';
import path from 'node:path';
import type { Job } from '@peekaround/shared';
import type { EventHub } from '../events.js';

export interface JobContext {
  job: Job;
  signal: AbortSignal;
  progress(p: number): void;
  /** transfers: byte + file counters (progress follows bytes, or files when sizes are unknown) */
  transfer(t: { bytes?: { done: number; total: number }; files?: { done: number; total: number } }): void;
}

type Lane = 'media' | 'transfer';
const laneOf = (type: Job['type']): Lane => (type === 'transfer' ? 'transfer' : 'media');

export type JobRunner = (jc: JobContext) => Promise<Job['result']>;

/** job assets (overlay PNGs, music) may only use these extensions */
const ASSET_EXT = new Set(['png', 'jpg', 'jpeg', 'webp', 'mp3', 'm4a', 'aac', 'ogg', 'opus', 'wav', 'flac', 'bin']);

/**
 * Serial job queues with two independent lanes:
 *  - 'media': video-edit / slideshow (one ffmpeg job at a time — they already use all cores)
 *  - 'transfer': device copies + remote (MEGA) imports, one at a time, never waiting for an export
 */
export class JobService {
  private jobs = new Map<string, Job>();
  private queues: Record<Lane, { job: Job; run: JobRunner }[]> = { media: [], transfer: [] };
  private running: Record<Lane, { job: Job; ctrl: AbortController } | null> = { media: null, transfer: null };
  private runs = new Set<Promise<void>>();
  private closed = false;
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
      ...(type === 'transfer' ? { bytes: { done: 0, total: 0 }, files: { done: 0, total: 0 } } : {}),
      error: null,
      result: null,
      createdAt: Date.now(),
    };
    this.jobs.set(job.id, job);
    const lane = laneOf(type);
    if (this.closed) {
      job.status = 'canceled';
      return job;
    }
    this.queues[lane].push({ job, run });
    this.emit(job);
    this.start(lane);
    return job;
  }

  private start(lane: Lane) {
    if (this.closed) return;
    const p: Promise<void> = this.pump(lane).finally(() => this.runs.delete(p));
    this.runs.add(p);
  }

  /** Shutdown: cancel queued jobs, abort running ones and wait for them to wind down. */
  async close() {
    this.closed = true;
    for (const lane of ['media', 'transfer'] as Lane[]) {
      for (const q of this.queues[lane]) q.job.status = 'canceled';
      this.queues[lane] = [];
      this.running[lane]?.ctrl.abort();
    }
    let timer: NodeJS.Timeout | undefined;
    await Promise.race([Promise.allSettled([...this.runs]), new Promise<void>((r) => (timer = setTimeout(r, 10_000)))]);
    clearTimeout(timer);
  }

  cancel(id: string) {
    const j = this.jobs.get(id);
    if (!j) return;
    const lane = laneOf(j.type);
    const running = this.running[lane];
    if (running?.job.id === id) running.ctrl.abort();
    else if (j.status === 'queued') {
      this.queues[lane] = this.queues[lane].filter((q) => q.job.id !== id);
      j.status = 'canceled';
      this.emit(j);
    }
  }

  clearFinished() {
    for (const [id, j] of this.jobs) if (j.status !== 'queued' && j.status !== 'running') this.jobs.delete(id);
  }

  async saveAsset(data: AsyncIterable<Buffer> | Buffer, ext = 'png'): Promise<string> {
    await fsp.mkdir(this.assetDir, { recursive: true });
    const safeExt = ASSET_EXT.has(ext.toLowerCase()) ? ext.toLowerCase() : 'bin';
    const id = `${crypto.randomUUID()}.${safeExt}`;
    const file = this.assetPath(id);
    if (Buffer.isBuffer(data)) await fsp.writeFile(file, data);
    else {
      const chunks: Buffer[] = [];
      for await (const c of data) chunks.push(c);
      await fsp.writeFile(file, Buffer.concat(chunks));
    }
    return id;
  }

  assetPath(id: string): string {
    if (!/^[0-9a-f-]{36}\.[a-z0-9]{1,5}$/i.test(id)) throw new Error('Invalid asset id');
    const file = path.resolve(this.assetDir, id);
    if (!file.startsWith(path.resolve(this.assetDir) + path.sep)) throw new Error('Invalid asset id');
    return file;
  }

  /** resolves when the job finished (any final status) — tests and chained work */
  async wait(id: string): Promise<Job | undefined> {
    for (;;) {
      const j = this.jobs.get(id);
      if (!j || (j.status !== 'queued' && j.status !== 'running')) return j ? { ...j } : undefined;
      await new Promise<void>((resolve) => {
        const off = this.events.on((e) => {
          if (e.type === 'job' && e.job.id === id && e.job.status !== 'queued' && e.job.status !== 'running') {
            off();
            resolve();
          }
        });
      });
    }
  }

  private emit(job: Job) {
    this.events.emit({
      type: 'job',
      job: { ...job, ...(job.bytes ? { bytes: { ...job.bytes } } : {}), ...(job.files ? { files: { ...job.files } } : {}) },
    });
  }

  private async pump(lane: Lane) {
    if (this.running[lane]) return;
    const next = this.queues[lane].shift();
    if (!next) return;
    const ctrl = new AbortController();
    this.running[lane] = { job: next.job, ctrl };
    const job = next.job;
    job.status = 'running';
    this.emit(job);
    let last = 0;
    const tick = () => {
      const now = Date.now();
      if (now - last > 300) {
        last = now;
        this.emit(job);
      }
    };
    try {
      job.result = await next.run({
        job,
        signal: ctrl.signal,
        progress: (p) => {
          job.progress = Math.max(0, Math.min(1, p));
          tick();
        },
        transfer: (t) => {
          if (t.bytes) job.bytes = { ...t.bytes };
          if (t.files) job.files = { ...t.files };
          const b = job.bytes;
          const f = job.files;
          if (b && b.total > 0) job.progress = Math.max(0, Math.min(1, b.done / b.total));
          else if (f && f.total > 0) job.progress = Math.max(0, Math.min(1, f.done / f.total));
          tick();
        },
      });
      job.status = 'done';
      job.progress = 1;
    } catch (e: any) {
      job.status = ctrl.signal.aborted ? 'canceled' : 'error';
      job.error = ctrl.signal.aborted ? null : String(e?.message ?? e).slice(0, 500);
    } finally {
      this.running[lane] = null;
      this.emit(job);
      this.start(lane);
    }
  }
}
