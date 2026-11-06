import { createRequire } from 'node:module';
import { Worker } from 'node:worker_threads';

/**
 * HEIC/HEIF decoding (heic-decode = libheif compiled to WebAssembly) off the main thread.
 * A 12 MP iPhone photo takes ~1 s of pure WASM; on the main thread that froze every request (and in
 * the desktop app the window's IPC) for that long, once per HEIC file of a library. One worker
 * serializes the decodes (it is also the CPU budget for them) and quits when idle for a while.
 * If the worker cannot start (exotic packaging), decoding falls back to the calling thread.
 */

export interface DecodedImage {
  width: number;
  height: number;
  data: Uint8Array;
}

const WORKER_SRC = `
const { parentPort, workerData } = require('node:worker_threads');
const decode = require(workerData.mod);
parentPort.on('message', async ({ id, buf }) => {
  try {
    const img = await decode({ buffer: buf });
    const data = new Uint8Array(img.data.buffer, img.data.byteOffset, img.data.byteLength);
    parentPort.postMessage({ id, width: img.width, height: img.height, data }, [data.buffer]);
  } catch (e) {
    parentPort.postMessage({ id, error: String((e && e.message) || e) });
  }
});
`;

const IDLE_MS = 30_000;

let worker: Worker | null = null;
let broken = false;
let seq = 0;
let idle: NodeJS.Timeout | null = null;
const pending = new Map<number, { resolve: (v: DecodedImage) => void; reject: (e: Error) => void }>();

function failAll(e: Error) {
  for (const p of pending.values()) p.reject(e);
  pending.clear();
}

function getWorker(): Worker | null {
  if (worker) return worker;
  if (broken) return null;
  try {
    const mod = createRequire(import.meta.url).resolve('heic-decode');
    const w = new Worker(WORKER_SRC, { eval: true, workerData: { mod } });
    w.unref();
    w.on('message', (m: { id: number; width?: number; height?: number; data?: Uint8Array; error?: string }) => {
      const p = pending.get(m.id);
      if (!p) return;
      pending.delete(m.id);
      if (m.error || !m.data) p.reject(new Error(m.error ?? 'HEIC decode failed'));
      else p.resolve({ width: m.width!, height: m.height!, data: m.data });
      if (!pending.size) armIdle();
    });
    w.on('error', (e) => {
      // the module could not load in a worker: decode on the calling thread from now on
      if (!pending.size || /Cannot find module|ERR_/.test(String(e))) broken = true;
      failAll(e instanceof Error ? e : new Error(String(e)));
      worker = null;
    });
    w.on('exit', () => {
      if (worker === w) worker = null;
      failAll(new Error('HEIC worker exited'));
    });
    worker = w;
    return w;
  } catch {
    broken = true;
    return null;
  }
}

function armIdle() {
  if (idle) clearTimeout(idle);
  idle = setTimeout(() => {
    idle = null;
    if (!pending.size && worker) {
      const w = worker;
      worker = null;
      void w.terminate();
    }
  }, IDLE_MS);
  idle.unref();
}

async function decodeHere(buf: Buffer): Promise<DecodedImage> {
  const { default: decode } = await import('heic-decode');
  const img = await decode({ buffer: buf });
  return { width: img.width, height: img.height, data: new Uint8Array(img.data.buffer, img.data.byteOffset, img.data.byteLength) };
}

/** RGBA pixels of the first image of a HEIC/HEIF file */
export async function decodeHeic(buf: Buffer): Promise<DecodedImage> {
  const w = getWorker();
  if (!w) return decodeHere(buf);
  if (idle) {
    clearTimeout(idle);
    idle = null;
  }
  const id = ++seq;
  try {
    return await new Promise<DecodedImage>((resolve, reject) => {
      pending.set(id, { resolve, reject });
      w.postMessage({ id, buf });
    });
  } catch (e) {
    if (broken) return decodeHere(buf);
    throw e;
  }
}

/** stop the worker (server shutdown / tests) */
export async function closeHeicWorker() {
  if (idle) clearTimeout(idle);
  idle = null;
  const w = worker;
  worker = null;
  if (w) await w.terminate();
}
