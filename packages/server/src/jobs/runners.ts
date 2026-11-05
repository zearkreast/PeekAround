import crypto from 'node:crypto';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { joinRel, type Job, type SlideshowExportSpec, type VideoEditSpec } from '@peekaround/shared';
import { getSettings, type Ctx } from '../context.js';
import type { MediaRow } from '../media/mapper.js';
import { pool } from '../media/scanner.js';
import { exists, HttpError, normalizeRel, uniqueEditPath, uniquePath } from '../paths.js';
import { folderPath, mediaAbs, mediaRow } from '../routes/util.js';
import { AbortError, listEncoders, pipeFrames, runFfmpeg, spawnFed, type FeedProcess } from './ffmpeg-process.js';
import type { JobContext } from './index.js';
import {
  buildEncoderArgs,
  buildPieceArgs,
  canvasSize,
  frameBytes,
  normalizeSlideshowSpec,
  pieceRange,
  planSlideshow,
  solidFrame,
  type PieceInput,
  type SlideshowPlan,
  type SlideSource,
} from './slideshow.js';
import {
  buildVideoEditPasses,
  normalizeVideoEditSpec,
  pickCodecs,
  planVideoEdit,
  type VideoEditPlan,
  type VideoSourceInfo,
} from './video-edit.js';

/**
 * Job runners for the video editor and the slideshow exporter.
 * Every request is validated (and the source probed) *before* it is queued, so bad
 * input fails the POST with a 400 instead of producing a failed job.
 */
export function registerJobRunners(ctx: Ctx) {
  return {
    videoEdit(spec: VideoEditSpec): Promise<Job> {
      return startVideoEdit(ctx, spec);
    },
    slideshow(spec: SlideshowExportSpec): Promise<Job> {
      return startSlideshow(ctx, spec);
    },
  };
}

const rand = () => crypto.randomBytes(4).toString('hex');

async function isFile(p: string): Promise<boolean> {
  try {
    return (await fsp.stat(p)).isFile();
  } catch {
    return false;
  }
}

function rowExtra(r: MediaRow): Record<string, unknown> {
  try {
    return r.extra ? (JSON.parse(r.extra) as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

// ───────────────────────── Video edit ─────────────────────────

export async function startVideoEdit(ctx: Ctx, spec: VideoEditSpec): Promise<Job> {
  const n = normalizeVideoEditSpec(spec);
  const row = mediaRow(ctx, n.sourceId);
  if (row.kind !== 'video') throw new HttpError(400, 'The source is not a video');
  const { folder, abs } = mediaAbs(ctx, row);
  if (!(await isFile(abs))) throw new HttpError(404, 'Source file not found');

  const probe = await ctx.ffmpeg.probe(abs);
  const extra = rowExtra(row);
  const src: VideoSourceInfo = {
    width: probe?.width ?? row.width ?? 0,
    height: probe?.height ?? row.height ?? 0,
    duration: probe?.duration ?? row.duration ?? 0,
    hasAudio: probe ? probe.hasAudio : extra.hasAudio === true,
    fps: probe?.fps ?? (typeof extra.fps === 'number' ? extra.fps : null),
  };
  const plan = planVideoEdit(n, src);

  const overlays = plan.overlays.map((o) => {
    try {
      return ctx.jobs.assetPath(o.assetId);
    } catch {
      throw new HttpError(400, 'Invalid overlay asset id');
    }
  });
  for (const o of overlays) if (!(await isFile(o))) throw new HttpError(400, 'Overlay asset not found — upload it first');

  const encoders = await listEncoders(ctx.ffmpeg);
  pickCodecs(plan.ext, encoders); // 400 when this ffmpeg can't write the format

  const suffix = getSettings(ctx).editSuffix;
  return ctx.jobs.enqueue('video-edit', row.name, (jc) =>
    runVideoEdit(ctx, jc, { plan, abs, overlays, encoders, folderId: folder.id, dir: row.dir, base: path.parse(row.name).name, suffix }),
  );
}

async function runVideoEdit(
  ctx: Ctx,
  jc: JobContext,
  o: {
    plan: VideoEditPlan;
    abs: string;
    overlays: string[];
    encoders: Set<string>;
    folderId: number;
    dir: string;
    base: string;
    suffix: string;
  },
): Promise<Job['result']> {
  const { plan } = o;
  if (!(await isFile(o.abs))) throw new Error('Source file not found');
  const dirAbs = path.dirname(o.abs);
  const target = await uniqueEditPath(dirAbs, o.base, o.suffix, plan.ext);
  const part = `${target}.${rand()}.part`;
  let palette: string | undefined;
  if (plan.ext === 'gif') {
    await fsp.mkdir(ctx.cfg.tempDir, { recursive: true });
    palette = path.join(ctx.cfg.tempDir, `palette-${rand()}.png`);
  }
  try {
    const passes = buildVideoEditPasses(plan, { input: o.abs, overlays: o.overlays, output: part, palette, encoders: o.encoders });
    let done = 0;
    for (const pass of passes) {
      await runFfmpeg(ctx.ffmpeg, pass.args, {
        signal: jc.signal,
        duration: plan.duration,
        onProgress: (p) => jc.progress((done + p * pass.weight) * 0.99),
      });
      done += pass.weight;
    }
    if (jc.signal.aborted) throw new AbortError();
    const st = await fsp.stat(part).catch(() => null);
    if (!st || st.size === 0) throw new Error('ffmpeg produced no output');
    // the name may have been taken while we were encoding
    const dest = (await exists(target)) ? await uniqueEditPath(dirAbs, o.base, o.suffix, plan.ext) : target;
    await fsp.rename(part, dest);
    const name = path.basename(dest);
    await ctx.scanner.upsertFile(o.folderId, joinRel(o.dir, name));
    return { folderId: String(o.folderId), dir: o.dir, name };
  } finally {
    await fsp.rm(part, { force: true }).catch(() => {});
    if (palette) await fsp.rm(palette, { force: true }).catch(() => {});
  }
}

// ───────────────────────── Slideshow ─────────────────────────

interface PreparedSlide {
  row: MediaRow;
  abs: string;
}

export async function startSlideshow(ctx: Ctx, spec: SlideshowExportSpec): Promise<Job> {
  const n = normalizeSlideshowSpec(spec);
  const slides: PreparedSlide[] = n.items.map((it) => {
    const row = mediaRow(ctx, it.id);
    if (row.kind !== 'image' && row.kind !== 'video') throw new HttpError(400, `"${row.name}" is not an image or a video`);
    return { row, abs: mediaAbs(ctx, row).abs };
  });

  const sources: SlideSource[] = new Array(slides.length);
  await pool(
    slides.map((s, i) => ({ s, i })),
    4,
    async ({ s, i }) => {
      if (!(await isFile(s.abs))) throw new HttpError(404, `File not found: ${s.row.name}`);
      if (s.row.kind === 'image') {
        sources[i] = { kind: 'image' };
        return;
      }
      const p = await ctx.ffmpeg.probe(s.abs);
      if (p && !p.width) throw new HttpError(400, `"${s.row.name}" has no video stream`);
      sources[i] = { kind: 'video', duration: p?.duration ?? s.row.duration ?? 0 };
    },
  );

  let music: string | null = null;
  if (n.audioId) {
    const row = mediaRow(ctx, n.audioId);
    if (row.kind !== 'audio' && row.kind !== 'video') throw new HttpError(400, 'The music track must be an audio file');
    const { abs } = mediaAbs(ctx, row);
    if (!(await isFile(abs))) throw new HttpError(404, 'Music file not found');
    const p = await ctx.ffmpeg.probe(abs);
    if (p && !p.hasAudio) throw new HttpError(400, `"${row.name}" has no audio track`);
    music = abs;
  }

  let folderId: number;
  let dir: string;
  let dirAbs: string;
  if (n.target) {
    const t = folderPath(ctx, n.target.folderId, n.target.dir);
    const st = await fsp.stat(t.abs).catch(() => null);
    if (!st?.isDirectory()) throw new HttpError(400, 'Target directory does not exist');
    folderId = t.folder.id;
    dir = normalizeRel(n.target.dir);
    dirAbs = t.abs;
  } else {
    const first = slides[0].row;
    folderId = first.folder_id;
    dir = first.dir;
    dirAbs = path.dirname(slides[0].abs);
  }

  const plan = planSlideshow(n, sources);
  const encoders = await listEncoders(ctx.ffmpeg);
  pickCodecs('mp4', encoders);

  return ctx.jobs.enqueue('slideshow', `${n.name}.mp4`, (jc) =>
    runSlideshow(ctx, jc, { plan, slides, music, folderId, dir, dirAbs, name: n.name }),
  );
}

async function runSlideshow(
  ctx: Ctx,
  jc: JobContext,
  o: { plan: SlideshowPlan; slides: PreparedSlide[]; music: string | null; folderId: number; dir: string; dirAbs: string; name: string },
): Promise<Job['result']> {
  const { plan } = o;
  const { signal } = jc;
  await fsp.mkdir(ctx.cfg.tempDir, { recursive: true });
  const tmp = await fsp.mkdtemp(path.join(ctx.cfg.tempDir, 'slideshow-'));
  const target = await uniquePath(o.dirAbs, `${o.name}.mp4`);
  const part = `${target}.${rand()}.part`;
  const fb = frameBytes(plan);
  const fill = solidFrame(plan.width, plan.height, plan.background);
  const totalBytes = plan.totalFrames * fb;
  let sent = 0;

  // Pre-rendered canvases (raw RGB) — at most the current, next and prefetched slide exist at once.
  const canvases = new Map<number, Promise<string>>();
  const canvas = (i: number): Promise<string> => {
    let pr = canvases.get(i);
    if (!pr) {
      pr = renderCanvas(ctx, plan, i, o.slides[i], tmp);
      pr.catch(() => {}); // awaited later; don't crash on an unobserved prefetch failure
      canvases.set(i, pr);
    }
    return pr;
  };
  const dropCanvas = async (i: number) => {
    const pr = canvases.get(i);
    if (!pr) return;
    canvases.delete(i);
    const file = await pr.catch(() => null);
    if (file) await fsp.rm(file, { force: true }).catch(() => {});
  };
  const input = async (i: number): Promise<PieceInput> => {
    const s = plan.slides[i];
    if (s.kind === 'video') return { kind: 'video', path: o.slides[i].abs };
    return { kind: 'canvas', path: await canvas(i), ...canvasSize(plan, s) };
  };
  const prefetch = (i: number) => {
    if (i < plan.slides.length && plan.slides[i].kind === 'image') void canvas(i);
  };

  let enc: FeedProcess | null = null;
  try {
    enc = spawnFed(ctx.ffmpeg, buildEncoderArgs(plan, { output: part, music: o.music }), signal);
    for (let i = 0; i < plan.slides.length; i++) {
      if (signal.aborted) throw new AbortError();
      const r = pieceRange(plan, i);
      prefetch(i + 1);
      const inputs = [await input(i)];
      if (r.xfade) inputs.push(await input(i + 1));
      prefetch(i + 2);
      const piece = buildPieceArgs(plan, i, inputs);
      await pipeFrames(ctx.ffmpeg, piece.args, enc, piece.frames * fb, fill, signal, (b) => {
        sent += b;
        jc.progress((sent / totalBytes) * 0.98);
      });
      await dropCanvas(i);
    }
    enc.stdin.end();
    await enc.done;
    if (signal.aborted) throw new AbortError();
    const st = await fsp.stat(part).catch(() => null);
    if (!st || st.size === 0) throw new Error('ffmpeg produced no output');
    const dest = (await exists(target)) ? await uniquePath(o.dirAbs, `${o.name}.mp4`) : target;
    await fsp.rename(part, dest);
    const name = path.basename(dest);
    await ctx.scanner.upsertFile(o.folderId, joinRel(o.dir, name));
    return { folderId: String(o.folderId), dir: o.dir, name };
  } finally {
    if (enc && !enc.exited()) {
      enc.stdin.destroy();
      enc.kill();
      await enc.done.catch(() => {});
    }
    await Promise.all([...canvases.values()].map((pr) => pr.catch(() => null)));
    await fsp.rm(tmp, { recursive: true, force: true }).catch(() => {});
    await fsp.rm(part, { force: true }).catch(() => {});
  }
}

/** Decode (EXIF-rotated / HEIC-capable), fit + pad onto the background and store as raw RGB. */
async function renderCanvas(ctx: Ctx, plan: SlideshowPlan, i: number, s: PreparedSlide, tmp: string): Promise<string> {
  const { width, height } = canvasSize(plan, plan.slides[i]);
  const src = await ctx.thumbs.source(s.abs, s.row, Math.max(width, height));
  if (!src) throw new Error(`Cannot decode ${s.row.name}`);
  const bg = plan.background;
  let out: { data: Buffer; info: { width: number; height: number; channels: number } };
  try {
    out = await src
      .flatten({ background: bg })
      .resize(width, height, { fit: 'contain', background: bg, kernel: 'lanczos3' })
      .toColourspace('srgb')
      .removeAlpha()
      .raw()
      .toBuffer({ resolveWithObject: true });
  } catch (e: any) {
    throw new Error(`Cannot decode ${s.row.name}: ${e?.message ?? e}`);
  }
  if (out.info.width !== width || out.info.height !== height || out.info.channels !== 3) {
    throw new Error(`Unexpected decode result for ${s.row.name}`);
  }
  const file = path.join(tmp, `slide-${i}.rgb`);
  await fsp.writeFile(file, out.data);
  return file;
}
