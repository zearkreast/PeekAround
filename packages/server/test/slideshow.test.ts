import { describe, expect, it } from 'vitest';
import type { SlideshowExportSpec } from '@peekaround/shared';
import { HttpError } from '../src/paths.js';
import {
  buildEncoderArgs,
  buildPieceArgs,
  canvasSize,
  frameBytes,
  kenBurnsExpr,
  KEN_BURNS_MOVES,
  normalizeHex,
  normalizeSlideshowSpec,
  pieceRange,
  planSlideshow,
  RANDOM_TRANSITIONS,
  solidFrame,
  XFADE_TRANSITIONS,
  type PieceInput,
  type SlideSource,
} from '../src/jobs/slideshow.js';

function spec(extra: Partial<SlideshowExportSpec> = {}): SlideshowExportSpec {
  return {
    items: [{ id: '1' }, { id: '2' }, { id: '3' }],
    duration: 2,
    transition: 'fade',
    transitionDuration: 0.5,
    kenBurns: false,
    width: 1280,
    height: 720,
    fps: 30,
    background: '#000',
    ...extra,
  };
}

const IMG: SlideSource = { kind: 'image' };
const plan = (extra: Partial<SlideshowExportSpec> = {}, sources?: SlideSource[], rnd?: () => number) => {
  const n = normalizeSlideshowSpec(spec(extra));
  return planSlideshow(n, sources ?? n.items.map(() => IMG), rnd);
};

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

const canvas = (p: ReturnType<typeof plan>, i: number, path = `/tmp/s${i}.rgb`): PieceInput => ({
  kind: 'canvas',
  path,
  ...canvasSize(p, p.slides[i]),
});

describe('normalizeSlideshowSpec', () => {
  it('normalizes and defaults', () => {
    const n = normalizeSlideshowSpec({ items: [{ id: 5 }], width: 1279, height: 721 });
    expect(n).toMatchObject({
      items: [{ id: '5', duration: null, trim: null }],
      duration: 4,
      transition: 'fade',
      fps: 30,
      background: '#000000',
      name: 'Slayt',
      audioId: null,
      target: null,
    });
    expect(n.width % 2).toBe(0);
    expect(n.height % 2).toBe(0);
  });

  it('sanitizes the name', () => {
    expect(normalizeSlideshowSpec(spec({ name: 'Tatil: 2024/Yaz.mp4' })).name).toBe('Tatil_ 2024_Yaz');
  });

  it('rejects bad input with 400', () => {
    expect400(() => normalizeSlideshowSpec(null));
    expect400(() => normalizeSlideshowSpec(spec({ items: [] })), /items/);
    expect400(() => normalizeSlideshowSpec(spec({ items: [{ id: 'x/../1' }] })), /id/);
    expect400(() => normalizeSlideshowSpec(spec({ transition: 'wobble' as never })), /transition/);
    expect400(() => normalizeSlideshowSpec(spec({ fps: 500 })), /fps/);
    expect400(() => normalizeSlideshowSpec(spec({ width: 10 })), /width/);
    expect400(() => normalizeSlideshowSpec(spec({ duration: Number.POSITIVE_INFINITY })), /duration/);
    expect400(() => normalizeSlideshowSpec(spec({ background: 'red' })), /background/);
    expect400(() => normalizeSlideshowSpec(spec({ items: [{ id: '1', trim: { start: 4, end: 2 } }] })), /trim/);
  });

  it('normalizes hex colours', () => {
    expect(normalizeHex('#AbC')).toBe('#aabbcc');
    expect(normalizeHex('112233')).toBe('#112233');
  });
});

describe('planSlideshow', () => {
  it('accumulates transition offsets (total = Σ durations − cuts · transition)', () => {
    const p = plan();
    expect(p.slides.map((s) => s.frames)).toEqual([60, 60, 60]);
    expect(p.cuts).toEqual([
      { frames: 15, transition: 'fade' },
      { frames: 15, transition: 'fade' },
    ]);
    expect(p.totalFrames).toBe(180 - 30);
    expect(p.duration).toBeCloseTo(5);
    // the xfade offset of cut i on the global timeline is Σ durations[0..i] − (i+1)·T
    let start = 0;
    for (let i = 0; i < p.cuts.length; i++) {
      const r = pieceRange(p, i);
      expect((start + r.frames - r.xfade!.frames) / p.fps).toBeCloseTo((i + 1) * 2 - (i + 1) * 0.5);
      start += r.frames;
    }
  });

  it('uses per-item durations and clamps transitions to half a slide', () => {
    const p = plan({ items: [{ id: '1', duration: 0.4 }, { id: '2' }, { id: '3', duration: 1 }], transitionDuration: 2 });
    expect(p.slides.map((s) => s.frames)).toEqual([12, 60, 30]);
    expect(p.cuts.map((c) => c.frames)).toEqual([6, 15]);
    expect(p.totalFrames).toBe(102 - 21);
  });

  it('none → hard cuts', () => {
    const p = plan({ transition: 'none' });
    expect(p.cuts.every((c) => c.frames === 0 && c.transition === null)).toBe(true);
    expect(p.totalFrames).toBe(180);
    expect(pieceRange(p, 1).xfade).toBeNull();
  });

  it('maps transition names to xfade', () => {
    expect(XFADE_TRANSITIONS).toEqual({
      fade: 'fade',
      slide: 'slideleft',
      zoom: 'zoomin',
      blur: 'hblur',
      flip: 'squeezev',
      circle: 'circleopen',
      kenburns: 'fade',
    });
    for (const [ours, theirs] of Object.entries(XFADE_TRANSITIONS)) {
      expect(plan({ transition: ours as never }).cuts[0].transition).toBe(theirs);
    }
  });

  it('random picks a different transition per cut from the pool', () => {
    let k = 0;
    const seq = [0, 0, 0.5, 0.99];
    const p = plan({ transition: 'random', items: [1, 2, 3, 4, 5].map((id) => ({ id: String(id) })) }, undefined, () => seq[k++ % seq.length]);
    const names = p.cuts.map((c) => c.transition!);
    for (const n of names) expect(RANDOM_TRANSITIONS).toContain(n);
    for (let i = 1; i < names.length; i++) expect(names[i]).not.toBe(names[i - 1]);
  });

  it('kenburns transition implies Ken Burns motion, alternating moves', () => {
    const p = plan({ transition: 'kenburns' });
    expect(p.slides.map((s) => s.kenBurns)).toEqual(KEN_BURNS_MOVES.slice(0, 3));
    expect(p.kbScale).toBe(2);
    expect(canvasSize(p, p.slides[0])).toEqual({ width: 2560, height: 1440 });
    expect(plan({ width: 3840, height: 2160, kenBurns: true }).kbScale).toBe(2);
    expect(plan({ width: 4096, height: 2160, kenBurns: true }).kbScale).toBe(1);
    expect(plan().kbScale).toBe(1);
  });

  it('plans video items from their trim', () => {
    const p = plan(
      { items: [{ id: '1' }, { id: '2', trim: { start: 1.5, end: 4 } }, { id: '3' }] },
      [IMG, { kind: 'video', duration: 10 }, { kind: 'video', duration: 3 }],
    );
    expect(p.slides[1]).toMatchObject({ kind: 'video', frames: 75, videoStart: 1.5, kenBurns: null });
    expect(p.slides[2]).toMatchObject({ kind: 'video', frames: 90, videoStart: 0 });
    expect400(() => plan({ items: [{ id: '1', trim: { start: 9.99, end: 12 } }] }, [{ kind: 'video', duration: 10 }]), /short/);
  });
});

describe('piece args', () => {
  it('static image piece ending in a transition', () => {
    const p = plan();
    const { args, frames } = buildPieceArgs(p, 1, [canvas(p, 1), canvas(p, 2)]);
    expect(frames).toBe(45); // 60 − 15 shared with the previous transition
    const s = args.join(' ');
    expect(s).toContain('-f rawvideo -pix_fmt rgb24 -video_size 1280x720 -framerate 30 -i /tmp/s1.rgb');
    expect(s).toContain('-i /tmp/s2.rgb');
    expect(s).toContain('tpad=stop_mode=clone:stop=-1,trim=end_frame=45[a]');
    expect(s).toContain('trim=end_frame=15[b]');
    expect(s).toContain('[a][b]xfade=transition=fade:duration=0.5:offset=1[v]');
    expect(s).toContain('-frames:v 45');
    expect(s.endsWith('-f rawvideo -pix_fmt yuv420p pipe:1')).toBe(true);
  });

  it('last piece has no transition', () => {
    const p = plan();
    const { args, frames } = buildPieceArgs(p, 2, [canvas(p, 2)]);
    expect(frames).toBe(45);
    expect(args.join(' ')).not.toContain('xfade');
  });

  it('ken burns piece continues the move where the previous piece stopped', () => {
    const p = plan({ kenBurns: true });
    const s = buildPieceArgs(p, 1, [canvas(p, 1), canvas(p, 2)]).args.join(' ');
    expect(s).toContain('-video_size 2560x1440');
    expect(s).toContain('format=yuv444p,zoompan=');
    expect(s).toContain('d=45:s=1280x720:fps=30');
    expect(s).toContain('(on+15)/59');
    expect(s).toContain('d=15:s=1280x720:fps=30');
  });

  it('ken burns expressions', () => {
    const e = kenBurnsExpr({ zoomFrom: 1, zoomTo: 1.2, xFrom: 0.5, xTo: 0.5, yFrom: 0, yTo: 1 }, 10, 101);
    expect(e.z).toBe('(1+0.2*((on+10)/100))');
    expect(e.x).toBe('(iw-iw/zoom)*0.5');
    expect(e.y).toBe('(ih-ih/zoom)*(0+1*((on+10)/100))');
  });

  it('video piece seeks into the trimmed clip and pads with the background', () => {
    const p = plan(
      { items: [{ id: '1', trim: { start: 2, end: 5 } }, { id: '2' }], background: '#102030' },
      [{ kind: 'video', duration: 10 }, IMG],
    );
    const s = buildPieceArgs(p, 0, [{ kind: 'video', path: '/v/clip.mp4' }, canvas(p, 1)]).args.join(' ');
    expect(s).toContain('-ss 2 -i /v/clip.mp4');
    expect(s).toContain('[0:V:0]setpts=PTS-STARTPTS,scale=1280:720:force_original_aspect_ratio=decrease');
    expect(s).toContain('pad=1280:720:(ow-iw)/2:(oh-ih)/2:color=0x102030');
    expect(s).toContain('-an');
    // second piece starts 15 frames (0.5 s) into slide 2 — for a video that is a later seek
    const p2 = plan({ items: [{ id: '1' }, { id: '2', trim: { start: 2, end: 5 } }] }, [IMG, { kind: 'video', duration: 10 }]);
    expect(buildPieceArgs(p2, 1, [{ kind: 'video', path: '/v/clip.mp4' }]).args.join(' ')).toContain('-ss 2.5 -i /v/clip.mp4');
  });

  it('piece frames add up to the total', () => {
    const p = plan({ items: [{ id: '1', duration: 1 }, { id: '2' }, { id: '3', duration: 3 }, { id: '4' }], transitionDuration: 0.7 });
    let sum = 0;
    for (let i = 0; i < p.slides.length; i++) sum += pieceRange(p, i).frames;
    expect(sum).toBe(p.totalFrames);
  });
});

describe('encoder args', () => {
  it('loops, trims and fades the music to the video length', () => {
    const p = plan();
    const a = buildEncoderArgs(p, { output: '/out/s.mp4.part', music: '/m/song.mp3' }).join(' ');
    expect(a).toContain('-f rawvideo -pix_fmt yuv420p -video_size 1280x720 -framerate 30 -i pipe:0');
    expect(a).toContain('-stream_loop -1 -i /m/song.mp3');
    expect(a).toContain('[1:a:0]atrim=end=5,asetpts=PTS-STARTPTS,afade=t=out:st=3:d=2[a]');
    expect(a).toContain('-c:v libx264');
    expect(a).toContain('-c:a aac');
    expect(a).toContain('-movflags +faststart -f mp4 /out/s.mp4.part');
  });

  it('no music → video only', () => {
    const a = buildEncoderArgs(plan(), { output: '/o.mp4' });
    expect(a.join(' ')).not.toContain('-c:a');
    expect(a).toContain('0:v');
  });
});

describe('solidFrame', () => {
  it('produces BT.709 limited-range yuv420p', () => {
    const black = solidFrame(4, 2, '#000000');
    expect(black.length).toBe(frameBytes({ width: 4, height: 2 }));
    expect([...black]).toEqual([...Array(8).fill(16), ...Array(4).fill(128)]);
    const white = solidFrame(2, 2, '#fff');
    expect([...white]).toEqual([235, 235, 235, 235, 128, 128]);
    const red = solidFrame(2, 2, '#ff0000');
    expect(red[0]).toBe(63);
    expect(red[4]).toBe(102);
    expect(red[5]).toBe(240);
  });
});
