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

export class Ffmpeg {
  available = true;
  probeAvailable = true;

  constructor(
    public ffmpegPath: string,
    public ffprobePath: string,
  ) {}

  async check(): Promise<void> {
    this.available = await canRun(this.ffmpegPath, ['-version']);
    this.probeAvailable = await canRun(this.ffprobePath, ['-version']);
  }

  /** Run ffmpeg to completion, collecting stdout as a Buffer. */
  run(args: string[], opts: { timeoutMs?: number; input?: Buffer } = {}): Promise<Buffer> {
    return runCollect(this.ffmpegPath, ['-hide_banner', '-loglevel', 'error', ...args], opts);
  }

  spawn(args: string[]): ChildProcess {
    return spawn(this.ffmpegPath, ['-hide_banner', ...args], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
  }

  async probe(file: string): Promise<ProbeResult | null> {
    if (!this.probeAvailable) return null;
    try {
      const out = await runCollect(
        this.ffprobePath,
        ['-v', 'quiet', '-print_format', 'json', '-show_format', '-show_streams', file],
        { timeoutMs: 20000 },
      );
      const j = JSON.parse(out.toString('utf8'));
      const v = (j.streams ?? []).find((s: any) => s.codec_type === 'video' && !s.disposition?.attached_pic);
      const a = (j.streams ?? []).find((s: any) => s.codec_type === 'audio');
      let rotation = 0;
      if (v) {
        const sd = (v.side_data_list ?? []).find((d: any) => d.rotation !== undefined);
        rotation = Number(sd?.rotation ?? v.tags?.rotate ?? 0) || 0;
        rotation = ((Math.round(rotation / 90) * 90) % 360 + 360) % 360;
      }
      let width = v ? Number(v.width) || null : null;
      let height = v ? Number(v.height) || null : null;
      if (width && height && (rotation === 90 || rotation === 270)) [width, height] = [height, width];
      const fpsStr: string | undefined = v?.avg_frame_rate || v?.r_frame_rate;
      let fps: number | null = null;
      if (fpsStr && fpsStr.includes('/')) {
        const [n, d] = fpsStr.split('/').map(Number);
        if (d) fps = Math.round((n / d) * 100) / 100;
      }
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

function canRun(bin: string, args: string[]): Promise<boolean> {
  return new Promise((resolve) => {
    try {
      const p = spawn(bin, args, { stdio: 'ignore', windowsHide: true });
      p.on('error', () => resolve(false));
      p.on('exit', (code) => resolve(code === 0));
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
