import type { ChildProcess } from 'node:child_process';
import type { Writable } from 'node:stream';
import type { Ffmpeg } from '../ffmpeg.js';

/**
 * Process plumbing shared by the video-edit and slideshow jobs:
 * spawning ffmpeg with `-progress`, abort handling and encoder detection.
 * (The pure argument builders live in video-edit.ts / slideshow.ts.)
 */

export class AbortError extends Error {
  constructor() {
    super('Canceled');
    this.name = 'AbortError';
  }
}

/**
 * Incremental parser for `-progress pipe:1` output (key=value lines).
 * Returns the newest output time in seconds, or null when the chunk had none.
 */
export class ProgressParser {
  private rest = '';

  feed(chunk: string): number | null {
    const text = this.rest + chunk;
    const lines = text.split('\n');
    this.rest = lines.pop() ?? '';
    let latest: number | null = null;
    for (const raw of lines) {
      const line = raw.trim();
      const eq = line.indexOf('=');
      if (eq < 0) continue;
      const key = line.slice(0, eq);
      const val = line.slice(eq + 1);
      // out_time_ms is (despite the name) in microseconds as well — an old ffmpeg quirk.
      if (key === 'out_time_us' || key === 'out_time_ms') {
        const us = Number(val);
        if (Number.isFinite(us) && us >= 0) latest = us / 1e6;
      } else if (key === 'out_time' && latest === null) {
        const t = parseClock(val);
        if (t !== null) latest = t;
      }
    }
    return latest;
  }
}

/** "00:01:02.500000" → 62.5 */
export function parseClock(v: string): number | null {
  const m = /^(-?\d+):(\d{2}):(\d{2}(?:\.\d+)?)$/.exec(v.trim());
  if (!m) return null;
  const t = Number(m[1]) * 3600 + Number(m[2]) * 60 + Number(m[3]);
  return Number.isFinite(t) && t >= 0 ? t : null;
}

/** Keep the last `max` characters of a process' stderr for error messages. */
function tail(buf: string, add: string, max = 6000): string {
  const s = buf + add;
  return s.length > max ? s.slice(s.length - max) : s;
}

function ffError(stderr: string, code: number | null, sig: NodeJS.Signals | null): Error {
  const lines = stderr
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);
  const msg = lines.slice(-4).join(' | ') || `ffmpeg exited with ${code ?? sig}`;
  return new Error(`ffmpeg failed: ${msg}`.slice(0, 480));
}

export interface RunOptions {
  signal: AbortSignal;
  /** expected output duration in seconds (for progress) */
  duration?: number;
  /** 0..1 within this run */
  onProgress?: (p: number) => void;
}

/**
 * Run ffmpeg to completion. Adds `-nostdin -progress pipe:1 -nostats -loglevel error`.
 * SIGKILLs the process when `signal` aborts and rejects with AbortError.
 */
export function runFfmpeg(ff: Ffmpeg, args: string[], opts: RunOptions): Promise<void> {
  if (opts.signal.aborted) return Promise.reject(new AbortError());
  return new Promise<void>((resolve, reject) => {
    let p: ChildProcess;
    try {
      p = ff.spawn(['-nostdin', '-loglevel', 'error', '-progress', 'pipe:1', '-nostats', ...args]);
    } catch (e) {
      reject(e);
      return;
    }
    let stderr = '';
    const parser = new ProgressParser();
    const kill = () => {
      try {
        p.kill('SIGKILL');
      } catch {
        /* already gone */
      }
    };
    opts.signal.addEventListener('abort', kill, { once: true });
    p.stdin?.end();
    p.stdout?.setEncoding('utf8');
    p.stdout?.on('data', (c: string) => {
      const t = parser.feed(c);
      if (t !== null && opts.onProgress && opts.duration && opts.duration > 0) opts.onProgress(Math.min(1, t / opts.duration));
    });
    p.stderr?.setEncoding('utf8');
    p.stderr?.on('data', (c: string) => {
      stderr = tail(stderr, c);
    });
    p.on('error', (e) => {
      opts.signal.removeEventListener('abort', kill);
      reject(e);
    });
    p.on('close', (code, sig) => {
      opts.signal.removeEventListener('abort', kill);
      if (opts.signal.aborted) reject(new AbortError());
      else if (code === 0) resolve();
      else reject(ffError(stderr, code, sig));
    });
  });
}

/** A long-running ffmpeg whose stdin we feed (the slideshow encoder). */
export interface FeedProcess {
  stdin: Writable;
  /** resolves when the process exits successfully */
  done: Promise<void>;
  /** set once the process has exited (successfully or not) */
  exited: () => boolean;
  kill(): void;
}

export function spawnFed(ff: Ffmpeg, args: string[], signal: AbortSignal): FeedProcess {
  if (signal.aborted) throw new AbortError();
  const p = ff.spawn(['-loglevel', 'error', '-nostats', ...args]);
  let stderr = '';
  let exited = false;
  const kill = () => {
    try {
      p.kill('SIGKILL');
    } catch {
      /* already gone */
    }
  };
  signal.addEventListener('abort', kill, { once: true });
  p.stderr?.setEncoding('utf8');
  p.stderr?.on('data', (c: string) => {
    stderr = tail(stderr, c);
  });
  p.stdout?.resume();
  // Writes after the process died raise EPIPE — surfaced through `done` instead.
  p.stdin!.on('error', () => {});
  const done = new Promise<void>((resolve, reject) => {
    p.on('error', (e) => {
      exited = true;
      signal.removeEventListener('abort', kill);
      reject(e);
    });
    p.on('close', (code, sig) => {
      exited = true;
      signal.removeEventListener('abort', kill);
      if (signal.aborted) reject(new AbortError());
      else if (code === 0) resolve();
      else reject(ffError(stderr, code, sig));
    });
  });
  done.catch(() => {}); // observed by the caller; avoid unhandled rejections while it is busy elsewhere
  return { stdin: p.stdin!, done, exited: () => exited, kill };
}

/**
 * Run an ffmpeg that writes raw frames to stdout and forward exactly `expectedBytes`
 * of them into `sink` (honouring back-pressure). Missing bytes (a source that produced
 * fewer frames than planned) are filled from `fill`, which must be one frame.
 */
export function pipeFrames(
  ff: Ffmpeg,
  args: string[],
  sink: FeedProcess,
  expectedBytes: number,
  fill: Buffer,
  signal: AbortSignal,
  onBytes: (n: number) => void,
): Promise<void> {
  if (signal.aborted) return Promise.reject(new AbortError());
  return new Promise<void>((resolve, reject) => {
    const p = ff.spawn(['-nostdin', '-loglevel', 'error', '-nostats', ...args]);
    let stderr = '';
    let written = 0;
    let settled = false;
    const kill = () => {
      try {
        p.kill('SIGKILL');
      } catch {
        /* already gone */
      }
    };
    const finish = (err?: Error) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', onAbort);
      if (err) {
        kill();
        reject(err);
      } else resolve();
    };
    const onAbort = () => finish(new AbortError());
    signal.addEventListener('abort', onAbort, { once: true });
    sink.done.then(
      () => finish(new Error('encoder exited early')),
      (e) => finish(e instanceof Error ? e : new Error(String(e))),
    );
    p.stdin?.end();
    p.stderr?.setEncoding('utf8');
    p.stderr?.on('data', (c: string) => {
      stderr = tail(stderr, c);
    });
    /** returns false when the sink wants us to wait for 'drain' */
    const write = (buf: Buffer): boolean => {
      if (!buf.length) return true;
      written += buf.length;
      onBytes(buf.length);
      return sink.stdin.write(buf);
    };
    p.stdout!.on('data', (chunk: Buffer) => {
      if (settled) return;
      const room = expectedBytes - written;
      if (room <= 0) return;
      if (!write(chunk.length > room ? chunk.subarray(0, room) : chunk)) {
        p.stdout!.pause();
        sink.stdin.once('drain', () => p.stdout!.resume());
      }
    });
    p.on('error', (e) => finish(e));
    p.on('close', (code, sig) => {
      if (settled) return;
      if (signal.aborted) return finish(new AbortError());
      if (code !== 0) return finish(ffError(stderr, code, sig));
      // pad a short piece so the encoder's frame count (and A/V sync) stays exact
      const pad = () => {
        while (!settled) {
          const missing = expectedBytes - written;
          if (missing <= 0) return finish();
          const off = written % fill.length;
          if (!write(fill.subarray(off, off + Math.min(missing, fill.length - off)))) {
            sink.stdin.once('drain', pad);
            return;
          }
        }
      };
      pad();
    });
  });
}

const encoderCache = new WeakMap<Ffmpeg, Promise<Set<string>>>();

/** Names of the encoders this ffmpeg build offers (cached per binary). */
export function listEncoders(ff: Ffmpeg): Promise<Set<string>> {
  let p = encoderCache.get(ff);
  if (!p) {
    p = ff
      .run(['-encoders'], { timeoutMs: 15000 })
      .then((buf) => parseEncoders(buf.toString('utf8')))
      .catch(() => new Set<string>());
    encoderCache.set(ff, p);
  }
  return p;
}

export function parseEncoders(text: string): Set<string> {
  const out = new Set<string>();
  for (const line of text.split('\n')) {
    const m = /^\s*([VAS][A-Z.]{5})\s+(\S+)/.exec(line);
    if (m && m[2] !== '=') out.add(m[2]);
  }
  return out;
}
