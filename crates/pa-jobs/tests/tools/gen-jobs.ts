// Golden fixtures for pa-jobs: runs the TypeScript builders (video-edit.ts, slideshow.ts) on
// generated specs and records every result (normalized spec / plan / ffmpeg args) or 400 error.
// Run: cd /home/user/peekaround && node_modules/.bin/tsx <this file> <out.json>
import fs from 'node:fs';
import zlib from 'node:zlib';
import * as VE from '/home/user/peekaround/packages/server/src/jobs/video-edit.ts';
import * as SS from '/home/user/peekaround/packages/server/src/jobs/slideshow.ts';

const out = process.argv[2];

let seed = 0x2545f491;
const rnd = () => {
  seed |= 0;
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const ri = (a: number, b: number) => Math.floor(a + rnd() * (b - a + 1));
const pick = <T,>(a: T[]): T => a[Math.floor(rnd() * a.length)];
const maybe = <T,>(p: number, v: () => T): T | undefined => (rnd() < p ? v() : undefined);

/** a value that is usually valid, sometimes a string / out of range / garbage */
const numish = (lo: number, hi: number, int = false): unknown => {
  const r = rnd();
  const v = lo + rnd() * (hi - lo);
  const x = int ? Math.round(v) : Math.round(v * 1000) / 1000;
  if (r < 0.86) return x;
  if (r < 0.9) return String(x);
  if (r < 0.92) return hi + 1 + rnd() * 10;
  if (r < 0.94) return lo - 1 - rnd() * 10;
  if (r < 0.95) return null;
  if (r < 0.96) return '';
  if (r < 0.97) return 'abc';
  if (r < 0.98) return true;
  return ' 1e1 ';
};

const err = (e: any) => ({ error: { status: e?.statusCode ?? 500, message: String(e?.message ?? e) } });
const run = <T,>(f: () => T): T | { error: { status: number; message: string } } => {
  try {
    return f();
  } catch (e) {
    return err(e);
  }
};

const ASSET = () => `${'0123456789abcdef'.repeat(3).slice(0, 8)}-aaaa-4bbb-8ccc-${String(ri(100000000000, 999999999999))}.${pick(['png', 'png', 'webp', 'jpg', 'PNG'])}`;

function videoSpec(): unknown {
  if (rnd() < 0.02) return pick([null, 5, 'x', [], {}]);
  const s: Record<string, unknown> = {};
  s.sourceId = rnd() < 0.95 ? (rnd() < 0.5 ? String(ri(1, 9999)) : ri(1, 9999)) : pick(['a1', 1.5, null, '-3']);
  if (rnd() < 0.5) s.trim = rnd() < 0.95 ? { start: numish(0, 20), end: numish(0.5, 60) } : pick([true, 'x', {}]);
  if (rnd() < 0.4) s.crop = rnd() < 0.95 ? { x: numish(0, 600), y: numish(0, 400), w: numish(1, 1900), h: numish(1, 1100) } : pick([true, {}]);
  if (rnd() < 0.4) s.rotate = rnd() < 0.85 ? pick([0, 90, 180, 270]) : pick([45, '90', -90, null]);
  if (rnd() < 0.3) s.flipH = pick([true, false, 1, 0, 'x', '']);
  if (rnd() < 0.3) s.flipV = pick([true, false, 1]);
  if (rnd() < 0.5) s.speed = numish(0.25, 4);
  if (rnd() < 0.4) s.volume = numish(0, 2);
  if (rnd() < 0.3) s.mute = pick([true, false]);
  if (rnd() < 0.5) {
    s.adjust = rnd() < 0.95
      ? {
          brightness: maybe(0.6, () => numish(-1, 1)),
          contrast: maybe(0.6, () => numish(-1, 1)),
          saturation: maybe(0.6, () => numish(-1, 1)),
          hue: maybe(0.6, () => numish(-180, 180)),
          warmth: maybe(0.6, () => numish(-1, 1)),
          vignette: maybe(0.6, () => numish(0, 1)),
        }
      : pick([5, 'x', null]);
  }
  if (rnd() < 0.4) {
    s.overlays = rnd() < 0.95
      ? Array.from({ length: ri(0, 4) }, () => (rnd() < 0.95 ? { assetId: ASSET(), start: maybe(0.8, () => numish(0, 30)), end: numish(0, 40) } : pick([null, 'x', { assetId: 5 }])))
      : pick(['x', {}, Array.from({ length: 49 }, () => ({ assetId: ASSET(), end: 1 }))]);
  }
  if (rnd() < 0.9) {
    s.output = {
      format: rnd() < 0.95 ? pick(['mp4', 'webm', 'gif', undefined]) : pick(['avi', null, 5]),
      quality: rnd() < 0.95 ? pick(['high', 'medium', 'low', undefined]) : pick(['best', null]),
      height: rnd() < 0.5 ? null : numish(16, 4320, true),
    };
  }
  return s;
}

const ENC = [
  ['libx264', 'aac', 'libvpx-vp9', 'libopus', 'gif'],
  ['libx264', 'libvpx', 'libvorbis'],
  ['libvpx-vp9'],
  [],
];

const video: unknown[] = [];
for (let i = 0; i < 3000; i++) {
  const spec = videoSpec();
  const norm = run(() => VE.normalizeVideoEditSpec(spec));
  const rec: Record<string, unknown> = { spec, norm };
  if (!('error' in (norm as object))) {
    const src = {
      width: rnd() < 0.97 ? ri(64, 3840) : 0,
      height: rnd() < 0.97 ? ri(64, 2160) : 0,
      duration: rnd() < 0.97 ? Math.round(rnd() * 60000) / 1000 : 0,
      hasAudio: rnd() < 0.7,
      fps: rnd() < 0.8 ? pick([24, 25, 29.97, 30, 59.94, 60, 120, 23.976]) : pick([null, 0, 5000]),
    };
    rec.src = src;
    const plan = run(() => VE.planVideoEdit(norm as VE.NormalizedVideoEdit, src));
    rec.plan = plan;
    if (!('error' in (plan as object))) {
      const p = plan as VE.VideoEditPlan;
      const encoders = pick(ENC);
      rec.encoders = encoders;
      rec.passes = run(() =>
        VE.buildVideoEditPasses(p, {
          input: '/in/src.mp4',
          overlays: p.overlays.map((_, k) => `/assets/o${k}.png`),
          output: '/out/x.part',
          palette: p.ext === 'gif' ? '/tmp/pal.png' : undefined,
          encoders: new Set(encoders),
        }),
      );
      rec.atempo = VE.atempoChain(p.speed);
    }
  }
  video.push(rec);
}

function slideSpec(): unknown {
  if (rnd() < 0.02) return pick([null, 5, 'x', {}, { items: [] }]);
  const n = rnd() < 0.97 ? ri(1, 8) : pick([0, 1001]);
  const s: Record<string, unknown> = {
    items: Array.from({ length: n }, () =>
      rnd() < 0.97
        ? {
            id: rnd() < 0.97 ? String(ri(1, 500)) : pick(['x', null, 1.5]),
            duration: maybe(0.4, () => numish(0.2, 3600)),
            trim: maybe(0.2, () => (rnd() < 0.95 ? { start: numish(0, 10), end: numish(0, 20) } : true)),
          }
        : pick([null, 'x', 5]),
    ),
  };
  if (rnd() < 0.7) s.duration = numish(0.2, 600);
  if (rnd() < 0.8) s.transition = rnd() < 0.95 ? pick(SS.SLIDE_TRANSITIONS) : pick(['wipe', 5, null]);
  if (rnd() < 0.7) s.transitionDuration = numish(0, 10);
  if (rnd() < 0.5) s.kenBurns = pick([true, false, 1, '']);
  if (rnd() < 0.3) s.audioId = rnd() < 0.9 ? String(ri(1, 99)) : pick(['', null, 'x', 7]);
  if (rnd() < 0.6) s.width = numish(64, 4096, true);
  if (rnd() < 0.6) s.height = numish(64, 4096, true);
  if (rnd() < 0.5) s.fps = numish(1, 60);
  if (rnd() < 0.5) s.background = pick(['#000', '#ffffff', 'abc', '#12AB9f', 'red', '', null, '#12345', ' #fff ', 255]);
  if (rnd() < 0.3) s.target = rnd() < 0.9 ? { folderId: pick(['3', 4, 'x']), dir: pick([undefined, 'a/b', 5]) } : pick([true, 'x']);
  if (rnd() < 0.5) s.name = pick(['Trip', ' Summer 2024.MP4 ', 'a/b:c', '', '..', 'x'.repeat(170), 'con', 5]);
  return s;
}

const slideshow: unknown[] = [];
for (let i = 0; i < 2500; i++) {
  const spec = slideSpec();
  const norm = run(() => SS.normalizeSlideshowSpec(spec));
  const rec: Record<string, unknown> = { spec, norm };
  if (!('error' in (norm as object))) {
    const nn = norm as SS.NormalizedSlideshow;
    const sources = nn.items.map(() => (rnd() < 0.7 ? { kind: 'image' as const } : { kind: 'video' as const, duration: rnd() < 0.95 ? Math.round(rnd() * 30000) / 1000 : 0 }));
    rec.sources = sources;
    const draws: number[] = Array.from({ length: 16 }, () => rnd());
    rec.draws = draws;
    let k = 0;
    const plan = run(() => SS.planSlideshow(nn, sources, () => draws[k++ % draws.length]));
    rec.plan = plan;
    if (!('error' in (plan as object))) {
      const p = plan as SS.SlideshowPlan;
      const input = (j: number): SS.PieceInput => {
        const sl = p.slides[j];
        return sl.kind === 'video' ? { kind: 'video', path: `/m/v${j}.mp4` } : { kind: 'canvas', path: `/t/slide-${j}.rgb`, ...SS.canvasSize(p, sl) };
      };
      rec.pieces = p.slides.map((_, j) => {
        const r = SS.pieceRange(p, j);
        const inputs = [input(j)];
        if (r.xfade) inputs.push(input(j + 1));
        return { range: r, piece: SS.buildPieceArgs(p, j, inputs), canvas: SS.canvasSize(p, p.slides[j]) };
      });
      rec.encoder = SS.buildEncoderArgs(p, { output: '/o/x.part', music: rnd() < 0.5 ? '/m/song.mp3' : null });
      rec.frameBytes = SS.frameBytes(p);
      const sf = SS.solidFrame(4, 2, p.background);
      rec.solid = [...sf];
    }
  }
  slideshow.push(rec);
}

const kb = SS.KEN_BURNS_MOVES.flatMap((m) => [0, 3, 17].map((from) => ({ m, from, total: 90, e: SS.kenBurnsExpr(m, from, 90) })));
const fx = [0, 0.5, 2, 1 / 3, -1e-7, 0.125, 1.005, 2.675, 123.456789, -2.5, 1e-4, 99.9995, 0.0005, 3.0000001].flatMap((x) => [2, 3, 4, 6].map((d) => ({ x, d, s: VE.f(x, d) })));
const colors = Array.from({ length: 80 }, () => {
  const a = {
    brightness: Math.round((rnd() * 2 - 1) * 1e4) / 1e4,
    contrast: rnd() < 0.3 ? 0 : Math.round((rnd() * 2 - 1) * 1e4) / 1e4,
    saturation: rnd() < 0.3 ? 0 : rnd() * 2 - 1,
    hue: rnd() < 0.3 ? 0 : rnd() * 360 - 180,
    warmth: rnd() < 0.3 ? 0 : rnd() * 2 - 1,
    vignette: rnd() < 0.3 ? 0 : rnd(),
  };
  return { a, out: VE.colorFilters(a) };
});

fs.writeFileSync(out, zlib.gzipSync(JSON.stringify({ video, slideshow, kb, fx, colors }), { level: 9 }));
console.log(`wrote ${video.length} video + ${slideshow.length} slideshow cases`);
