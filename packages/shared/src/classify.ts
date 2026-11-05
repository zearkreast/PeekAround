/**
 * Media type classification (screenshot / selfie / panorama / animated) — pure.
 * Returns a MEDIA_FLAG bitmask. Videos are a kind, not a flag.
 */
import { MEDIA_FLAG } from './index';

export interface ClassifyInput {
  name: string;
  /** folder-relative dir ('/' separated) */
  dir?: string;
  ext: string;
  kind: string;
  width?: number | null;
  height?: number | null;
  make?: string | null;
  model?: string | null;
  lens?: string | null;
  /** XMP GPano: ProjectionType / UsePanoramaViewer present and equirectangular / true */
  gpano?: boolean;
  /** frames (sharp metadata `pages`) */
  pages?: number | null;
}

const fold = (s: string) =>
  s
    .replace(/[İIı]/g, 'i')
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .replace(/[’‘`´]/g, "'")
    .toLowerCase();

/** file name prefixes / substrings used by phones and desktop OSes for screenshots */
const SCREENSHOT_NAME = [
  'screenshot', // Android/en, Windows "Screenshot 2024-…", "Screenshot_20240101-…"
  'screen shot', // macOS (old)
  'screen_shot',
  'screen-shot',
  'screencap',
  'scrnshot',
  'ekran goruntusu', // tr (Windows/macOS)
  'ekran resmi', // tr macOS "Ekran Resmi 2024-…"
  'ekran_goruntusu',
  'bildschirmfoto', // de
  'bildschirmaufnahme',
  "capture d'ecran", // fr
  'capture_d_ecran',
  'captura de pantalla', // es
  'captura_de_pantalla',
  'schermafbeelding', // nl
  'istantanea schermo', // it
  'zrzut ekranu', // pl
  'снимок экрана', // ru (Windows)
  'скриншот', // ru
  'スクリーンショット', // ja
  '屏幕截图', // zh
  '截屏',
  '스크린샷', // ko
];
const SCREENSHOT_DIR = ['screenshots', 'screenshot', 'ekran goruntuleri', 'ekran goruntusu', 'bildschirmfotos', "captures d'ecran", 'capturas de pantalla', 'скриншоты', 'スクリーンショット'];

export function isScreenshotName(name: string, dir = ''): boolean {
  const n = fold(name);
  if (SCREENSHOT_NAME.some((p) => n.includes(p))) return true;
  const segs = dir.split('/').filter(Boolean).map(fold);
  return segs.some((s) => SCREENSHOT_DIR.includes(s));
}

export function classify(i: ClassifyInput): number {
  let f = 0;
  const ext = i.ext.toLowerCase();
  const w = i.width ?? 0;
  const h = i.height ?? 0;
  const hasCamera = !!(i.make || i.model);
  const image = i.kind === 'image';

  if (image) {
    if (isScreenshotName(i.name, i.dir)) f |= MEDIA_FLAG.screenshot;
    else if (ext === 'png' && !hasCamera && w > 0 && h > 0) {
      // phone-like portrait/landscape aspect (16:9 … 22:9) without any camera EXIF
      const a = Math.max(w, h) / Math.min(w, h);
      if (a >= 1.7 && a <= 2.45 && Math.min(w, h) >= 640) f |= MEDIA_FLAG.screenshot;
    }
  }

  if (image || i.kind === 'video') {
    const lens = `${i.lens ?? ''} ${i.model ?? ''}`.toLowerCase();
    if (/\bfront\b/.test(lens) || /selfie/i.test(i.name) || /^img_\d{8}_\d{6}_?\d*_?selfie/i.test(i.name)) f |= MEDIA_FLAG.selfie;
  }

  if (image) {
    if (i.gpano) f |= MEDIA_FLAG.panorama;
    else if (w > 0 && h > 0 && (hasCamera || /pano/i.test(i.name))) {
      const a = Math.max(w / h, h / w);
      if (a >= 2.3) f |= MEDIA_FLAG.panorama;
    }
    if (ext === 'gif' || ((ext === 'webp' || ext === 'png' || ext === 'apng' || ext === 'avif') && (i.pages ?? 1) > 1)) f |= MEDIA_FLAG.animated;
  }
  // screenshots are never panoramas (long scrolling captures)
  if (f & MEDIA_FLAG.screenshot) f &= ~MEDIA_FLAG.panorama;
  return f;
}

// ───────────────────────── Camera names ─────────────────────────

const MAKER_ALIASES: [RegExp, string][] = [
  [/^nikon\b.*/i, 'Nikon'],
  [/^canon\b.*/i, 'Canon'],
  [/^sony\b.*/i, 'Sony'],
  [/^samsung\b.*/i, 'Samsung'],
  [/^apple\b.*/i, 'Apple'],
  [/^olympus\b.*/i, 'Olympus'],
  [/^om digital solutions\b.*/i, 'OM System'],
  [/^fujifilm\b.*|^fuji photo film\b.*/i, 'Fujifilm'],
  [/^panasonic\b.*/i, 'Panasonic'],
  [/^pentax\b.*|^ricoh imaging\b.*/i, 'Pentax'],
  [/^ricoh\b.*/i, 'Ricoh'],
  [/^leica\b.*/i, 'Leica'],
  [/^hmd global\b.*/i, 'Nokia'],
  [/^huawei\b.*/i, 'Huawei'],
  [/^xiaomi\b.*/i, 'Xiaomi'],
  [/^oneplus\b.*/i, 'OnePlus'],
  [/^google\b.*/i, 'Google'],
  [/^lg electronics\b.*|^lge$/i, 'LG'],
  [/^motorola\b.*/i, 'Motorola'],
  [/^gopro\b.*/i, 'GoPro'],
  [/^dji\b.*/i, 'DJI'],
  [/^htc\b.*/i, 'HTC'],
  [/^oppo\b.*/i, 'OPPO'],
  [/^vivo\b.*/i, 'vivo'],
  [/^realme\b.*/i, 'realme'],
  [/^asus\b.*/i, 'ASUS'],
  [/^zte\b.*/i, 'ZTE'],
  [/^tcl\b.*/i, 'TCL'],
];

const SUFFIXES = /[\s,]+(corporation|corp\.?|co\.?,?\s*ltd\.?|company|ltd\.?|inc\.?|imaging|electronics|optical|digital|camera|gmbh|ag|s\.?a\.?)$/i;

function titleCase(s: string): string {
  // only touch SHOUTING names; keep "iPhone", "OnePlus", "EOS R5" as they are
  if (s !== s.toUpperCase() || !/[A-Z]{3,}/.test(s)) return s;
  return s.toLowerCase().replace(/(^|[\s\-/])([a-z])/g, (_, a: string, b: string) => a + b.toUpperCase());
}

export function normalizeMake(make: string | null | undefined): string | null {
  // real makers are short; the cap keeps SUFFIXES (a `[\s,]+…$` search) from going quadratic on hostile EXIF
  let m = (make ?? '').replace(/\0/g, '').trim().slice(0, 80).trim();
  if (!m) return null;
  for (const [re, name] of MAKER_ALIASES) if (re.test(m)) return name;
  for (let i = 0; i < 3; i++) m = m.replace(SUFFIXES, '').trim();
  return m ? titleCase(m) : null;
}

/**
 * Normalise EXIF Make/Model: "NIKON CORPORATION" + "NIKON D750" → Nikon / D750,
 * "samsung" + "SM-G991B" → Samsung / SM-G991B, "Canon" + "Canon EOS R5" → Canon / EOS R5.
 */
export function normalizeCamera(make: string | null | undefined, model: string | null | undefined): { make: string | null; model: string | null; key: string | null } {
  const mk = normalizeMake(make);
  let md = (model ?? '').replace(/\0/g, '').trim();
  if (md) {
    const rawMake = (make ?? '').replace(/\0/g, '').trim();
    const firstWord = rawMake.split(/[\s,]+/)[0] ?? '';
    for (const p of [rawMake, mk ?? '', firstWord].filter((x) => x.length >= 2)) {
      if (md.toLowerCase().startsWith(p.toLowerCase()) && md.length > p.length) {
        md = md.slice(p.length).replace(/^[\s\-_]+/, '');
        break;
      }
    }
  }
  if (!md) return { make: mk, model: null, key: null };
  return { make: mk, model: md, key: cameraKey(mk, md) };
}

export function cameraKey(make: string | null, model: string): string {
  return `${(make ?? '').toLowerCase()}|${model.toLowerCase()}`;
}
