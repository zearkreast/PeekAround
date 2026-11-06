import { spawn, type ChildProcess } from 'node:child_process';

export interface ProbeResult {
  duration: number | null;
  width: number | null;
  height: number | null;
  rotation: number;
  codec: string | null;
  audioCodec: string | null;
  fps: number | null;
  bitrate: number | null;
  container: string | null;
  creationTime: number | null;
  hasAudio: boolean;
  /** container + video stream tags, lower-cased keys (location, com.apple.quicktime.*, com.android.*) */
  tags: Record<string, string>;
}

export interface ProbeOptions {
  /** HTTP headers for URL inputs (ffmpeg "-headers", CRLF terminated lines) */
  headers?: string;
  timeoutMs?: number;
}

export class Ffmpeg {
  available = true;
  /** a working ffprobe binary (Docker image); otherwise probe() parses `ffmpeg -i` output */
  probeAvailable = true;

  constructor(
    public ffmpegPath: string,
    public ffprobePath: string,
  ) {}

  async check(): Promise<void> {
    this.available = await canRun(this.ffmpegPath, ['-version']);
    this.probeAvailable = !!this.ffprobePath && (await canRun(this.ffprobePath, ['-version']));
  }

  /** Run ffmpeg to completion, collecting stdout as a Buffer. */
  run(args: string[], opts: { timeoutMs?: number; input?: Buffer } = {}): Promise<Buffer> {
    return runCollect(this.ffmpegPath, ['-hide_banner', '-loglevel', 'error', ...args], opts);
  }

  spawn(args: string[]): ChildProcess {
    return spawn(this.ffmpegPath, ['-hide_banner', ...args], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
  }

  /** Container + stream info: ffprobe when available, else parsed from `ffmpeg -i` (no ffprobe binary needed). */
  async probe(file: string, opts: ProbeOptions = {}): Promise<ProbeResult | null> {
    if (this.probeAvailable) return this.ffprobe(file, opts);
    if (!this.available) return null;
    try {
      const stderr = await runStderr(this.ffmpegPath, ['-hide_banner', ...(opts.headers ? ['-headers', opts.headers] : []), '-i', file], opts.timeoutMs ?? 20000);
      return parseFfmpegInfo(stderr);
    } catch {
      return null;
    }
  }

  async ffprobe(file: string, opts: ProbeOptions = {}): Promise<ProbeResult | null> {
    try {
      const out = await runCollect(
        this.ffprobePath,
        ['-v', 'quiet', ...(opts.headers ? ['-headers', opts.headers] : []), '-print_format', 'json', '-show_format', '-show_streams', file],
        { timeoutMs: opts.timeoutMs ?? 20000 },
      );
      return probeFromJson(JSON.parse(out.toString('utf8')));
    } catch {
      return null;
    }
  }

  /** Grab a single frame as PNG. */
  async frame(file: string, atSeconds: number, maxWidth = 1600): Promise<Buffer> {
    const args = [
      ...(atSeconds > 0 ? ['-ss', atSeconds.toFixed(3)] : []),
      '-i',
      file,
      '-frames:v',
      '1',
      '-vf',
      `scale='min(${maxWidth},iw)':-2`,
      '-f',
      'image2pipe',
      '-vcodec',
      'png',
      'pipe:1',
    ];
    const buf = await this.run(args, { timeoutMs: 30000 });
    if (!buf.length && atSeconds > 0) return this.frame(file, 0, maxWidth);
    return buf;
  }
}

export function normRotation(r: number): number {
  return (((Math.round((Number(r) || 0) / 90) * 90) % 360) + 360) % 360;
}

function fpsOf(fpsStr: string | undefined): number | null {
  if (!fpsStr || !fpsStr.includes('/')) return null;
  const [n, d] = fpsStr.split('/').map(Number);
  return d ? Math.round((n / d) * 100) / 100 : null;
}

/** ffprobe -show_format -show_streams JSON → ProbeResult */
export function probeFromJson(j: any): ProbeResult {
  const v = (j.streams ?? []).find((s: any) => s.codec_type === 'video' && !s.disposition?.attached_pic);
  const a = (j.streams ?? []).find((s: any) => s.codec_type === 'audio');
  let rotation = 0;
  if (v) {
    const sd = (v.side_data_list ?? []).find((d: any) => d.rotation !== undefined);
    rotation = normRotation(Number(sd?.rotation ?? v.tags?.rotate ?? 0));
  }
  let width = v ? Number(v.width) || null : null;
  let height = v ? Number(v.height) || null : null;
  if (width && height && (rotation === 90 || rotation === 270)) [width, height] = [height, width];
  const fps = fpsOf(v?.avg_frame_rate) ?? fpsOf(v?.r_frame_rate);
  const tags: Record<string, string> = {};
  for (const src of [v?.tags, j.format?.tags]) {
    for (const [k, val] of Object.entries((src ?? {}) as Record<string, unknown>)) if (val != null) tags[k.toLowerCase()] = String(val);
  }
  const ct = j.format?.tags?.creation_time ?? v?.tags?.creation_time;
  const creationTime = ct ? Date.parse(ct) : NaN;
  const duration = Number(j.format?.duration ?? v?.duration ?? a?.duration);
  return {
    duration: Number.isFinite(duration) ? duration : null,
    width,
    height,
    rotation,
    codec: v?.codec_name ?? null,
    audioCodec: a?.codec_name ?? null,
    fps,
    bitrate: Number(j.format?.bit_rate) || null,
    container: j.format?.format_name ?? null,
    creationTime: Number.isFinite(creationTime) && creationTime > 0 ? creationTime : null,
    hasAudio: !!a,
    tags,
  };
}

interface ParsedStream {
  type: string;
  desc: string;
  tags: Record<string, string>;
  rotation: number | null;
}

/**
 * Parse the input description `ffmpeg -hide_banner -i <file>` prints on stderr (ffmpeg 4–7):
 *
 *   Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'x.mp4':
 *     Metadata:
 *       creation_time   : 2024-07-04T12:00:00Z
 *     Duration: 00:00:08.00, start: 0.000000, bitrate: 6940 kb/s
 *     Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 720x1280 [SAR 1:1 DAR 9:16], 6936 kb/s, 30 fps, 30 tbr, 15360 tbn (default)
 *       Metadata:
 *         rotate          : 90
 *       Side data:
 *         displaymatrix: rotation of -90.00 degrees
 *
 * Mirrors probeFromJson (ffprobe): first non-cover video stream, first audio stream, container
 * tags + video stream tags with lower-cased keys (container wins), creation_time, rotation.
 * Returns null when there is no "Input #0" (not a media file / unreadable).
 */
export function parseFfmpegInfo(stderr: string): ProbeResult | null {
  const lines = stderr.split(/\r?\n/);
  const at = lines.findIndex((l) => /^Input #0, /.test(l));
  if (at < 0) return null;
  const container = /^Input #0, (.+?), from '/.exec(lines[at])?.[1] ?? null;
  const format: Record<string, string> = {};
  const streams: ParsedStream[] = [];
  let duration: number | null = null;
  let bitrate: number | null = null;
  /** where "key : value" lines go, and the indent of the Metadata: header */
  let target: Record<string, string> | null = null;
  let targetIndent = -1;
  let sideData: ParsedStream | null = null;
  let lastKey: string | null = null;
  for (let i = at + 1; i < lines.length; i++) {
    const line = lines[i];
    if (/^(Input #[1-9]|Output #|Stream mapping:|At least one output)/.test(line) || (!line.startsWith(' ') && line.trim())) break;
    const indent = line.length - line.trimStart().length;
    const t = line.trim();
    if (!t) continue;
    if (target && indent > targetIndent) {
      // values keep their trailing spaces (e.g. major_brand "qt  ")
      const raw = line.trimStart();
      const cont = /^: ?(.*)$/.exec(raw);
      if (cont && lastKey) {
        target[lastKey] += `\n${cont[1]}`;
        continue;
      }
      const kv = /^([^:]+?)\s*: ?(.*)$/.exec(raw);
      if (kv) {
        lastKey = kv[1].trim().toLowerCase();
        target[lastKey] = kv[2];
        continue;
      }
    }
    target = null;
    lastKey = null;
    if (sideData && indent > 4) {
      const rot = /rotation of (-?[\d.]+) degrees/.exec(t);
      if (rot) sideData.rotation = Number(rot[1]);
      continue;
    }
    sideData = null;
    if (t === 'Metadata:') {
      target = indent <= 2 ? format : (streams[streams.length - 1]?.tags ?? null);
      targetIndent = indent;
      continue;
    }
    if (t === 'Side data:') {
      sideData = streams[streams.length - 1] ?? null;
      continue;
    }
    const d = /^Duration: (N\/A|(\d+):(\d+):(\d+(?:\.\d+)?))(?:, start: [-\d.]+)?, bitrate: (N\/A|(\d+) kb\/s)/.exec(t);
    if (d) {
      if (d[2] !== undefined) duration = Number(d[2]) * 3600 + Number(d[3]) * 60 + Number(d[4]);
      if (d[6] !== undefined) bitrate = Number(d[6]) * 1000;
      continue;
    }
    const st = /^Stream #0:\d+(?:\[[^\]]*\])?(?:\(([^)]*)\))?: (\w+): (.*)$/.exec(t);
    if (st) streams.push({ type: st[2].toLowerCase(), desc: st[3], tags: st[1] ? { language: st[1] } : {}, rotation: null });
  }
  const v = streams.find((s) => s.type === 'video' && !/\(attached pic\)/.test(s.desc));
  const a = streams.find((s) => s.type === 'audio');
  const codecOf = (s: ParsedStream | undefined) => (s ? (/^([\w-]+)/.exec(s.desc)?.[1] ?? null) : null);
  let width: number | null = null;
  let height: number | null = null;
  let fps: number | null = null;
  let rotation = 0;
  if (v) {
    const wh = /(?:^|[ ,])(\d{2,5})x(\d{2,5})(?=$|[ ,[])/.exec(v.desc);
    if (wh) [width, height] = [Number(wh[1]), Number(wh[2])];
    const f = /, ([\d.]+)(k?) fps\b/.exec(v.desc) ?? /, ([\d.]+)(k?) tbr\b/.exec(v.desc);
    if (f) fps = Math.round(Number(f[1]) * (f[2] ? 1000 : 1) * 100) / 100;
    rotation = normRotation(v.rotation ?? Number(v.tags.rotate ?? 0));
  }
  if (width && height && (rotation === 90 || rotation === 270)) [width, height] = [height, width];
  const tags: Record<string, string> = {};
  for (const src of [v?.tags, format]) for (const [k, val] of Object.entries(src ?? {})) tags[k] = val;
  const ct = format.creation_time ?? v?.tags.creation_time;
  const creationTime = ct ? Date.parse(ct) : NaN;
  return {
    duration,
    width,
    height,
    rotation,
    codec: codecOf(v),
    audioCodec: codecOf(a),
    fps,
    bitrate,
    container,
    creationTime: Number.isFinite(creationTime) && creationTime > 0 ? creationTime : null,
    hasAudio: !!a,
    tags,
  };
}

/** stderr of a process, whatever its exit code (`ffmpeg -i` without output exits 1) */
function runStderr(bin: string, args: string[], timeoutMs: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const p = spawn(bin, args, { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true });
    let err = '';
    const timer = setTimeout(() => p.kill('SIGKILL'), timeoutMs);
    p.stderr.on('data', (c) => {
      if (err.length < 256 * 1024) err += c.toString('utf8');
    });
    p.on('error', (e) => {
      clearTimeout(timer);
      reject(e);
    });
    p.on('close', () => {
      clearTimeout(timer);
      resolve(err);
    });
  });
}

function canRun(bin: string, args: string[]): Promise<boolean> {
  return new Promise((resolve) => {
    try {
      const p = spawn(bin, args, { stdio: 'ignore', windowsHide: true });
      // never let a hanging binary block server start-up
      const timer = setTimeout(() => {
        p.kill('SIGKILL');
        resolve(false);
      }, 10_000);
      timer.unref();
      p.on('error', () => (clearTimeout(timer), resolve(false)));
      p.on('exit', (code) => (clearTimeout(timer), resolve(code === 0)));
    } catch {
      resolve(false);
    }
  });
}

export function runCollect(bin: string, args: string[], opts: { timeoutMs?: number; input?: Buffer } = {}): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const p = spawn(bin, args, { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    const chunks: Buffer[] = [];
    let err = '';
    let timer: NodeJS.Timeout | undefined;
    if (opts.timeoutMs) timer = setTimeout(() => p.kill('SIGKILL'), opts.timeoutMs);
    p.stdout.on('data', (c) => chunks.push(c));
    p.stderr.on('data', (c) => {
      if (err.length < 4000) err += c.toString();
    });
    p.on('error', (e) => {
      if (timer) clearTimeout(timer);
      reject(e);
    });
    p.on('close', (code) => {
      if (timer) clearTimeout(timer);
      if (code === 0) resolve(Buffer.concat(chunks));
      else reject(new Error(`${bin} exited with ${code}: ${err.trim().slice(-800)}`));
    });
    if (opts.input) p.stdin.end(opts.input);
    else p.stdin.end();
  });
}

/** Video codecs + containers every modern browser can play natively. */
export function isBrowserPlayable(ext: string, codec: string | null): boolean {
  const okExt = ['mp4', 'm4v', 'mov', 'webm', 'ogv'];
  if (!okExt.includes(ext)) return false;
  if (!codec) return ext !== 'mov';
  return ['h264', 'vp8', 'vp9', 'av1', 'theora'].includes(codec);
}
