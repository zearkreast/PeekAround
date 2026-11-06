import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { Ffmpeg, parseFfmpegInfo, probeFromJson } from '../src/ffmpeg';

/**
 * The desktop app ships no ffprobe (−61 MB): probe() then parses `ffmpeg -hide_banner -i` stderr.
 * Samples in fixtures/ffmpeg-info were captured with ffmpeg 6.1 together with the ffprobe JSON of
 * the same file (lavfi test sources: rotated mp4, hevc mkv, mov with QuickTime location/make tags,
 * Android tags, vp9/opus webm, mpeg4 avi without audio + the repository's fixture videos).
 */
const DIR = path.join(__dirname, 'fixtures/ffmpeg-info');
const samples = fs.readdirSync(DIR).filter((f) => f.endsWith('.txt'));

describe('ffmpeg -i parser (no ffprobe)', () => {
  it('has samples', () => expect(samples.length).toBeGreaterThanOrEqual(10));

  for (const f of samples) {
    it(`matches ffprobe: ${f}`, () => {
      const parsed = parseFfmpegInfo(fs.readFileSync(path.join(DIR, f), 'utf8'))!;
      const ref = probeFromJson(JSON.parse(fs.readFileSync(path.join(DIR, f.replace(/\.txt$/, '.json')), 'utf8')));
      expect(parsed).not.toBeNull();
      // ffmpeg prints duration with 2 decimals and bit rates in kb/s
      expect(Math.abs((parsed.duration ?? 0) - (ref.duration ?? 0))).toBeLessThan(0.011);
      expect(Math.abs((parsed.bitrate ?? 0) - (ref.bitrate ?? 0))).toBeLessThan(1000);
      const { duration: _d, bitrate: _b, ...rest } = parsed;
      const { duration: _d2, bitrate: _b2, ...refRest } = ref;
      expect(rest).toEqual(refRest);
    });
  }

  it('reads rotation, QuickTime and Android tags', () => {
    const rot = parseFfmpegInfo(fs.readFileSync(path.join(DIR, 'rot90.mp4.txt'), 'utf8'))!;
    expect(rot.rotation).toBe(90);
    expect([rot.width, rot.height]).toEqual([240, 320]);
    const mov = parseFfmpegInfo(fs.readFileSync(path.join(DIR, 'apple.mov.txt'), 'utf8'))!;
    expect(mov.tags['com.apple.quicktime.location.iso6709']).toBe('+39.9208+032.8541+0938.000/');
    expect(mov.tags['com.apple.quicktime.creationdate']).toBe('2024-07-03T21:00:00+0300');
    expect(mov.tags['com.apple.quicktime.model']).toBe('iPhone 15');
    const and = parseFfmpegInfo(fs.readFileSync(path.join(DIR, 'android.mp4.txt'), 'utf8'))!;
    expect(and.tags['com.android.manufacturer']).toBe('Google');
    const mkv = parseFfmpegInfo(fs.readFileSync(path.join(DIR, 'hevc.mkv.txt'), 'utf8'))!;
    expect(mkv.codec).toBe('hevc');
    expect(mkv.fps).toBe(29.97);
    const avi = parseFfmpegInfo(fs.readFileSync(path.join(DIR, 'noaudio.avi.txt'), 'utf8'))!;
    expect(avi.hasAudio).toBe(false);
    expect(avi.audioCodec).toBeNull();
  });

  it('handles edge cases of other ffmpeg versions', () => {
    const txt = [
      "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'IMG_0001.MOV':",
      '  Metadata:',
      '    creation_time   : 2023-05-01T10:00:00.000000Z',
      '    comment         : line one',
      '                    : line two',
      '  Duration: N/A, start: 0.000000, bitrate: N/A',
      '  Stream #0:0(eng): Video: hevc (Main) (hvc1 / 0x31637668), yuv420p(tv, bt709), 1920x1080, 10000 kb/s, 29.98 fps, 30 tbr, 600 tbn (default)',
      '    Metadata:',
      '      rotate          : 90',
      '      creation_time   : 2023-05-01T10:00:00.000000Z',
      '  Stream #0:1(eng): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 128 kb/s (default)',
      '  Stream #0:2: Video: mjpeg (Baseline), yuvj420p(pc), 320x240, 90k tbr, 90k tbn (attached pic)',
      '  Stream #0:3[0x4](und): Video: h264, yuv420p, 1280x720, 1k fps, 1k tbr',
      '      Side data:',
      '        displaymatrix: rotation of -90.00 degrees',
      'At least one output file must be specified',
    ].join('\n');
    const p = parseFfmpegInfo(txt)!;
    expect(p.duration).toBeNull();
    expect(p.bitrate).toBeNull();
    expect(p.codec).toBe('hevc');
    expect(p.rotation).toBe(90); // rotate tag (old ffmpeg)
    expect([p.width, p.height]).toEqual([1080, 1920]);
    expect(p.fps).toBe(29.98);
    expect(p.tags.comment).toBe('line one\nline two');
    expect(p.tags.language).toBe('eng');
    expect(p.creationTime).toBe(Date.parse('2023-05-01T10:00:00Z'));
    expect(p.audioCodec).toBe('aac');
    // side data of a later stream does not leak into the first video stream
    const noRotate = parseFfmpegInfo(txt.replace('      rotate          : 90\n', ''))!;
    expect(noRotate.rotation).toBe(0);
    expect([noRotate.width, noRotate.height]).toEqual([1920, 1080]);
    expect(parseFfmpegInfo('garbage\n/x: No such file or directory')).toBeNull();
    expect(parseFfmpegInfo('')).toBeNull();
  });

  it('probe() falls back to the parser without ffprobe', async () => {
    const lib = path.join(__dirname, '../../../fixtures/library/Videolar');
    const file = path.join(lib, fs.readdirSync(lib).find((f) => f.endsWith('.mp4'))!);
    const noProbe = new Ffmpeg('ffmpeg', path.join(__dirname, 'no-such-ffprobe'));
    await noProbe.check();
    expect(noProbe.probeAvailable).toBe(false);
    if (!noProbe.available) return; // no ffmpeg on this machine
    const a = await noProbe.probe(file);
    expect(a?.codec).toBe('h264');
    expect(a?.duration).toBeGreaterThan(0);
    const withProbe = new Ffmpeg('ffmpeg', 'ffprobe');
    await withProbe.check();
    if (withProbe.probeAvailable) {
      const b = await withProbe.probe(file);
      expect([a?.width, a?.height, a?.rotation, a?.creationTime, a?.tags.location]).toEqual([b?.width, b?.height, b?.rotation, b?.creationTime, b?.tags.location]);
    }
    expect(await noProbe.probe(path.join(lib, 'missing.mp4'))).toBeNull();
  });
});
