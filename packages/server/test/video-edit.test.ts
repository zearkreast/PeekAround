import { describe, expect, it } from 'vitest';
import type { VideoEditSpec } from '@peekaround/shared';
import { HttpError } from '../src/paths.js';
import {
  atempoChain,
  audioGraph,
  buildVideoEditPasses,
  colorFilters,
  geometryFilters,
  normalizeVideoEditSpec,
  pickCodecs,
  planVideoEdit,
  videoGraph,
  type VideoSourceInfo,
} from '../src/jobs/video-edit.js';

const SRC: VideoSourceInfo = { width: 1920, height: 1080, duration: 10, hasAudio: true, fps: 30 };
const ALL = new Set(['libx264', 'aac', 'libvpx-vp9', 'libvpx', 'libopus', 'libvorbis', 'gif']);
const ASSET = '0b8f0a2e-1d0e-4a57-9a0e-2a9b8f3c1d11.png';

function spec(extra: Partial<VideoEditSpec> = {}): VideoEditSpec {
  return { sourceId: '7', output: { format: 'mp4', height: null, quality: 'medium' }, ...extra };
}

function plan(extra: Partial<VideoEditSpec> = {}, src: VideoSourceInfo = SRC) {
  return planVideoEdit(normalizeVideoEditSpec(spec(extra)), src);
}

function expect400(fn: () => unknown, msg?: RegExp) {
  try {
    fn();
  } catch (e) {
    expect(e).toBeInstanceOf(HttpError);
    expect((e as HttpError).statusCode).toBe(400);
    if (msg) expect((e as Error).message).toMatch(msg);
    return;
  }
  throw new Error('expected an HttpError(400)');
}

describe('normalizeVideoEditSpec', () => {
  it('fills defaults', () => {
    const n = normalizeVideoEditSpec({ sourceId: '3', output: { format: 'mp4', height: null, quality: 'high' } });
    expect(n).toMatchObject({ sourceId: '3', trim: null, crop: null, rotate: 0, speed: 1, volume: 1, mute: false, overlays: [] });
    expect(n.adjust).toEqual({ brightness: 0, contrast: 0, saturation: 0, hue: 0, warmth: 0, vignette: 0 });
  });

  it('rejects bad input with 400', () => {
    expect400(() => normalizeVideoEditSpec(null));
    expect400(() => normalizeVideoEditSpec({ output: {} }), /sourceId/);
    expect400(() => normalizeVideoEditSpec(spec({ sourceId: '../etc' })), /sourceId/);
    expect400(() => normalizeVideoEditSpec(spec({ speed: 8 })), /speed/);
    expect400(() => normalizeVideoEditSpec(spec({ speed: Number.NaN })), /speed/);
    expect400(() => normalizeVideoEditSpec(spec({ rotate: 45 as never })), /rotate/);
    expect400(() => normalizeVideoEditSpec(spec({ trim: { start: 3, end: 2 } })), /trim/);
    expect400(() => normalizeVideoEditSpec(spec({ crop: { x: -1, y: 0, w: 10, h: 10 } })), /crop/);
    expect400(() => normalizeVideoEditSpec(spec({ adjust: { hue: 500 } })), /hue/);
    expect400(() => normalizeVideoEditSpec(spec({ output: { format: 'avi' as never, height: null, quality: 'high' } })), /format/);
    expect400(() => normalizeVideoEditSpec(spec({ overlays: [{ assetId: '../../x.png', start: 0, end: 1 }] })), /PNG/);
  });

  it('drops overlays that are never visible', () => {
    const n = normalizeVideoEditSpec(
      spec({
        overlays: [
          { assetId: ASSET, start: 1, end: 1 },
          { assetId: ASSET, start: 0, end: 2 },
        ],
      }),
    );
    expect(n.overlays).toEqual([{ assetId: ASSET, start: 0, end: 2 }]);
  });
});

describe('planVideoEdit', () => {
  it('computes trim, speed and duration', () => {
    const p = plan({ trim: { start: 1, end: 3 }, speed: 2 });
    expect(p).toMatchObject({ start: 1, end: 3, trimmed: true, duration: 1, width: 1920, height: 1080 });
  });

  it('clamps the trim end to the source duration', () => {
    const p = plan({ trim: { start: 8, end: 20 } });
    expect(p.end).toBe(10);
    expect(p.duration).toBe(2);
    expect400(() => plan({ trim: { start: 10, end: 12 } }), /beyond/);
  });

  it('does not seek when the trim covers the whole clip', () => {
    expect(plan({ trim: { start: 0, end: 10 } }).trimmed).toBe(false);
  });

  it('crop → rotate → scale dimensions', () => {
    const p = plan({ crop: { x: 100, y: 50, w: 641, h: 481 }, rotate: 90 });
    expect(p.crop).toEqual({ x: 100, y: 50, w: 641, h: 481 });
    // rotated 90° → 481x641, made even
    expect([p.baseWidth, p.baseHeight]).toEqual([481, 641]);
    expect([p.width, p.height]).toEqual([480, 640]);

    const s = plan({ crop: { x: 0, y: 0, w: 1280, h: 720 }, output: { format: 'mp4', height: 360, quality: 'low' } });
    expect([s.width, s.height]).toEqual([640, 360]);
  });

  it('clips a crop that overhangs the frame and rejects one outside it', () => {
    const p = plan({ crop: { x: 1800, y: 1000, w: 400, h: 400 } });
    expect(p.crop).toEqual({ x: 1800, y: 1000, w: 120, h: 80 });
    expect400(() => plan({ crop: { x: 1920, y: 0, w: 10, h: 10 } }), /outside/);
  });

  it('drops a full-frame crop', () => {
    expect(plan({ crop: { x: 0, y: 0, w: 1920, h: 1080 } }).crop).toBeNull();
  });

  it('handles sources without audio, mute and gif', () => {
    expect(plan({}, { ...SRC, hasAudio: false }).audio).toBe(false);
    expect(plan({ mute: true }).audio).toBe(false);
    expect(plan({ output: { format: 'gif', height: 240, quality: 'high' } }).audio).toBe(false);
    expect(plan().audio).toBe(true);
  });

  it('caps the frame rate (gif ≤ 15, never above the source after a speed change)', () => {
    expect(plan({ output: { format: 'gif', height: null, quality: 'high' } }).fps).toBe(15);
    expect(plan({ output: { format: 'gif', height: null, quality: 'low' } }).fps).toBe(10);
    expect(plan({ output: { format: 'gif', height: null, quality: 'high' } }, { ...SRC, fps: 8 }).fps).toBe(8);
    expect(plan({ speed: 0.5, output: { format: 'gif', height: null, quality: 'high' } }, { ...SRC, fps: 24 }).fps).toBe(12);
    expect(plan({ speed: 4 }).fps).toBe(30);
    expect(plan({ speed: 0.5 }).fps).toBe(15);
    expect(plan({ speed: 2 }, { ...SRC, fps: null }).fps).toBeNull();
    expect(plan().fps).toBeNull();
  });

  it('rejects unreadable sources', () => {
    expect400(() => plan({}, { ...SRC, width: 0 }));
    expect400(() => plan({}, { ...SRC, duration: 0 }));
  });
});

describe('filters', () => {
  it('atempo steps stay within 0.5..2 and multiply to the speed', () => {
    for (const s of [0.25, 0.3, 0.5, 0.75, 1, 1.5, 2, 3, 4]) {
      const steps = atempoChain(s);
      for (const x of steps) {
        expect(x).toBeGreaterThanOrEqual(0.5 - 1e-9);
        expect(x).toBeLessThanOrEqual(2 + 1e-9);
      }
      expect(steps.reduce((a, b) => a * b, 1)).toBeCloseTo(s, 9);
    }
    expect(atempoChain(1)).toEqual([]);
    expect(atempoChain(4)).toEqual([2, 2]);
    expect(atempoChain(0.25)).toEqual([0.5, 0.5]);
  });

  it('applies geometry in order: crop, rotate, flip, scale, speed', () => {
    const p = plan({
      crop: { x: 10, y: 20, w: 640, h: 480 },
      rotate: 270,
      flipH: true,
      flipV: true,
      speed: 2,
      output: { format: 'mp4', height: 320, quality: 'medium' },
    });
    expect(geometryFilters(p)).toEqual([
      'crop=640:480:10:20:exact=1',
      'transpose=cclock',
      'hflip',
      'vflip',
      'scale=240:320:flags=bicubic',
      'setsar=1',
      'setpts=(PTS-STARTPTS)/2',
      'fps=30',
    ]);
    expect(geometryFilters(plan({ rotate: 180 }))).toEqual(['hflip', 'vflip', 'setsar=1', 'setpts=PTS-STARTPTS']);
    expect(geometryFilters(plan({ rotate: 90 }))[0]).toBe('transpose=clock');
  });

  it('maps colour adjustments to 5.1-compatible filters', () => {
    expect(colorFilters({ brightness: 0, contrast: 0, saturation: 0, hue: 0, warmth: 0, vignette: 0 })).toEqual([]);
    expect(colorFilters({ brightness: 0.5, contrast: -0.5, saturation: 1, hue: 30, warmth: 1, vignette: 0.5 })).toEqual([
      'eq=brightness=0.15:contrast=0.5:saturation=2',
      'hue=h=30',
      'colortemperature=temperature=4000:pl=1',
      'vignette=angle=0.6283',
    ]);
  });

  it('overlays full-frame PNGs on the output timeline', () => {
    const p = plan({
      speed: 2,
      overlays: [
        { assetId: ASSET, start: 0.5, end: 1.25 },
        { assetId: ASSET, start: 0, end: 3 },
      ],
      output: { format: 'mp4', height: 360, quality: 'medium' },
    });
    const g = videoGraph(p);
    expect(g.startsWith('[0:V:0]')).toBe(true);
    expect(g).toContain('[1:v]scale=640:360,format=rgba[ov0]');
    expect(g).toContain("[vb][ov0]overlay=0:0:enable='between(t,0.5,1.25)'[v0]");
    expect(g).toContain("[v0][ov1]overlay=0:0:enable='between(t,0,3)'[v1]");
    expect(g.endsWith('[v1]null[vo]')).toBe(true);
    // overlays come after the speed change
    expect(g.indexOf('setpts')).toBeLessThan(g.indexOf('overlay'));
  });

  it('builds the audio chain', () => {
    expect(audioGraph(plan({ speed: 4, volume: 0.5 }))).toBe('[0:a:0]asetpts=PTS-STARTPTS,atempo=2,atempo=2,volume=0.5[ao]');
    expect(audioGraph(plan({ mute: true }))).toBeNull();
  });
});

describe('buildVideoEditPasses', () => {
  const io = { input: '/in/clip.mov', overlays: [] as string[], output: '/out/clip_edit.mp4.part', encoders: ALL };

  it('mp4: input seek, x264 CRF, aac, faststart', () => {
    const [pass] = buildVideoEditPasses(plan({ trim: { start: 1, end: 3 }, output: { format: 'mp4', height: null, quality: 'high' } }), io);
    const a = pass.args;
    expect(a.slice(0, 7)).toEqual(['-y', '-ss', '1', '-t', '2', '-i', '/in/clip.mov']);
    expect(a.join(' ')).toContain('-c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p');
    expect(a.join(' ')).toContain('-c:a aac');
    expect(a.join(' ')).toContain('-movflags +faststart -f mp4');
    expect(a.at(-1)).toBe(io.output);
    expect(a).toContain('[ao]');
  });

  it('maps quality to CRF and drops audio when there is none', () => {
    const [low] = buildVideoEditPasses(plan({ output: { format: 'mp4', height: null, quality: 'low' } }, { ...SRC, hasAudio: false }), io);
    expect(low.args.join(' ')).toContain('-crf 28');
    expect(low.args).toContain('-an');
    expect(low.args.join(' ')).not.toContain('[0:a:0]');
  });

  it('webm: vp9 + opus, falls back to vp8 / vorbis', () => {
    const p = plan({ output: { format: 'webm', height: null, quality: 'medium' } });
    const [vp9] = buildVideoEditPasses(p, io);
    expect(vp9.args.join(' ')).toContain('-c:v libvpx-vp9 -crf 32 -b:v 0');
    expect(vp9.args.join(' ')).toContain('-c:a libopus');
    expect(vp9.args.join(' ')).toContain('-f webm');
    const [vp8] = buildVideoEditPasses(p, { ...io, encoders: new Set(['libvpx', 'libvorbis']) });
    expect(vp8.args.join(' ')).toContain('-c:v libvpx -crf 10');
    expect(vp8.args.join(' ')).toContain('-c:a libvorbis');
    expect400(() => pickCodecs('webm', new Set(['libx264'])), /WebM/);
    expect400(() => pickCodecs('mp4', new Set(['libvpx'])), /H\.264/);
  });

  it('gif: palette pass + paletteuse pass, no audio', () => {
    const p = plan({ overlays: [{ assetId: ASSET, start: 0, end: 1 }], output: { format: 'gif', height: 240, quality: 'medium' } });
    const passes = buildVideoEditPasses(p, { ...io, overlays: ['/tmp/ov.png'], palette: '/tmp/pal.png', output: '/out/x.gif.part' });
    expect(passes).toHaveLength(2);
    expect(passes[0].weight + passes[1].weight).toBeCloseTo(1);
    const [pal, enc] = passes.map((x) => x.args.join(' '));
    expect(pal).toContain('palettegen=stats_mode=diff');
    expect(pal).toContain('fps=12');
    expect(pal.endsWith('/tmp/pal.png')).toBe(true);
    // source, overlay, palette
    expect(enc).toContain('-i /in/clip.mov -i /tmp/ov.png -i /tmp/pal.png');
    expect(enc).toContain('[vo][2:v]paletteuse=');
    expect(enc).toContain('-an');
    expect(enc).toContain('-f gif');
  });

  it('requires one overlay path per planned overlay', () => {
    const p = plan({ overlays: [{ assetId: ASSET, start: 0, end: 1 }] });
    expect(() => buildVideoEditPasses(p, io)).toThrow();
  });
});
