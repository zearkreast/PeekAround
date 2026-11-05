import { execFileSync } from 'node:child_process';
import { describe, expect, it } from 'vitest';
import { Ffmpeg } from '../src/ffmpeg.js';
import { AbortError, parseClock, parseEncoders, ProgressParser, runFfmpeg } from '../src/jobs/ffmpeg-process.js';

function hasFfmpeg(): boolean {
  try {
    execFileSync('ffmpeg', ['-version'], { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

describe('ProgressParser', () => {
  it('reads out_time_us across chunk boundaries', () => {
    const p = new ProgressParser();
    expect(p.feed('frame=10\nfps=0.0\nout_time_us=12')).toBeNull();
    expect(p.feed('50000\nout_time_ms=1250000\nout_time=00:00:01.250000\nprogress=continue\n')).toBeCloseTo(1.25);
    expect(p.feed('out_time_us=N/A\nprogress=continue\n')).toBeNull();
    expect(p.feed('out_time_us=2500000\nprogress=end\n')).toBeCloseTo(2.5);
  });

  it('falls back to out_time', () => {
    expect(new ProgressParser().feed('out_time=00:01:02.500000\n')).toBeCloseTo(62.5);
    expect(parseClock('01:00:00.0')).toBe(3600);
    expect(parseClock('N/A')).toBeNull();
  });
});

describe('parseEncoders', () => {
  it('extracts encoder names', () => {
    const text = [
      'Encoders:',
      ' V..... = Video',
      ' A..... = Audio',
      ' ------',
      ' V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)',
      ' V....D libvpx-vp9           libvpx VP9 (codec vp9)',
      ' A....D aac                  AAC (Advanced Audio Coding)',
      ' A....D libopus              libopus Opus (codec opus)',
    ].join('\n');
    expect([...parseEncoders(text)].sort()).toEqual(['aac', 'libopus', 'libvpx-vp9', 'libx264']);
  });
});

describe.skipIf(!hasFfmpeg())('runFfmpeg', () => {
  const ff = new Ffmpeg('ffmpeg', 'ffprobe');

  it('reports progress against the expected duration', async () => {
    const seen: number[] = [];
    await runFfmpeg(ff, ['-f', 'lavfi', '-i', 'testsrc=s=64x64:r=25:d=2', '-f', 'null', '-'], {
      signal: new AbortController().signal,
      duration: 2,
      onProgress: (p) => seen.push(p),
    });
    expect(seen.length).toBeGreaterThan(0);
    expect(Math.max(...seen)).toBeGreaterThan(0.9);
    expect(Math.max(...seen)).toBeLessThanOrEqual(1);
  });

  it('rejects with the ffmpeg error message', async () => {
    await expect(
      runFfmpeg(ff, ['-i', '/definitely/not/here.mp4', '-f', 'null', '-'], { signal: new AbortController().signal }),
    ).rejects.toThrow(/ffmpeg failed: .*not\/here/);
  });

  it('kills ffmpeg on abort', async () => {
    const ctrl = new AbortController();
    const t0 = Date.now();
    const run = runFfmpeg(ff, ['-re', '-f', 'lavfi', '-i', 'testsrc=s=64x64:r=25:d=60', '-f', 'null', '-'], { signal: ctrl.signal });
    setTimeout(() => ctrl.abort(), 300);
    await expect(run).rejects.toBeInstanceOf(AbortError);
    expect(Date.now() - t0).toBeLessThan(10_000);
  });
});
