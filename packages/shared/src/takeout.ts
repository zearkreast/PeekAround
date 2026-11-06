/**
 * Google Takeout (Google Photos export) sidecars — pure.
 *
 * Takeout strips most EXIF from re-encoded files and puts capture time + location into a JSON next
 * to each file. Names Google produces (all seen in the wild):
 *   IMG_1234.jpg.json                         (classic)
 *   IMG_1234.jpg.supplemental-metadata.json   (2024+)
 *   IMG_1234.jpg.supplemental-metad.json      (whole name cut to 51 chars incl. ".json")
 *   Screenshot_2019…_Long app name.jp.json   (classic, cut as well)
 *   IMG_1234(1).jpg  ↔  IMG_1234.jpg(1).json / IMG_1234.jpg.supplemental-metadata(1).json
 *   IMG_1234-edited.jpg (also localised: -bearbeitet, -modifié, -düzenlendi …) → the original's JSON
 */

/** longest sidecar name Google writes, ".json" included */
export const TAKEOUT_MAX_NAME = 51;
const SUPPLEMENTAL = 'supplemental-metadata';
const EDITED = ['-edited', '-bearbeitet', '-modifié', '-modifie', '-düzenlendi', '-duzenlendi', '-editado', '-modificato', '-bewerkt', '-redigerad', '-edytowane', '-編集済み', '-已编辑', '-편집됨'];

export interface SidecarKey {
  /** media name the sidecar describes (possibly cut by Google's name limit) */
  target: string;
  /** duplicate index from "(n)", 0 = none */
  dup: number;
  /** the sidecar name was cut (prefix matching allowed) */
  truncated: boolean;
}

/** what a sidecar file name points at; null when it is not a sidecar-looking name */
export function parseSidecarName(jsonName: string): SidecarKey | null {
  if (!/\.json$/i.test(jsonName)) return null;
  let key = jsonName.slice(0, -5);
  let dup = 0;
  const dm = /\((\d{1,3})\)$/.exec(key);
  if (dm) {
    dup = Number(dm[1]);
    key = key.slice(0, -dm[0].length);
  }
  // Google keeps sidecar names (without "(n)") at about 46–51 characters: longer ones are cut
  const truncated = jsonName.length - (dm ? dm[0].length : 0) >= TAKEOUT_MAX_NAME - 5;
  // strip ".supplemental-metadata" or any cut prefix of it (".supplemental-metad", ".suppl", ".s");
  // no media extension is a prefix of that word, so this never eats a real extension
  const dot = key.lastIndexOf('.');
  if (dot > 0) {
    const tail = key.slice(dot + 1).toLowerCase();
    if (tail && SUPPLEMENTAL.startsWith(tail)) key = key.slice(0, dot);
  }
  if (!key) return null;
  return { target: key, dup, truncated };
}

const stemOf = (n: string) => {
  const d = n.lastIndexOf('.');
  return d > 0 ? n.slice(0, d) : n;
};

/** media name → base name without "(n)" + its duplicate index + edited-suffix variants */
export function mediaNameVariants(name: string): { names: string[]; dup: number } {
  const dot = name.lastIndexOf('.');
  let stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : '';
  let dup = 0;
  const dm = /\((\d{1,3})\)$/.exec(stem);
  if (dm) {
    dup = Number(dm[1]);
    stem = stem.slice(0, -dm[0].length);
  }
  const names = [stem + ext];
  const lower = stem.toLowerCase();
  for (const e of EDITED) {
    if (lower.endsWith(e)) {
      names.push(stem.slice(0, stem.length - e.length) + ext);
      break;
    }
  }
  return { names, dup };
}

/**
 * Pick the sidecar for `mediaName` from the JSON file names of its directory.
 * Exact targets win over cut ones; a cut target matches the start of the media name.
 */
export function matchSidecar(mediaName: string, jsonNames: Iterable<string>): string | null {
  const { names, dup } = mediaNameVariants(mediaName);
  let best: string | null = null;
  let bestScore = -1;
  for (const j of jsonNames) {
    const k = parseSidecarName(j);
    if (!k || k.dup !== dup) continue;
    for (let vi = 0; vi < names.length; vi++) {
      const n = names[vi];
      let score = -1;
      if (k.target === n) score = 100 - vi;
      else if (k.target.toLowerCase() === n.toLowerCase()) score = 90 - vi;
      else if (k.truncated && k.target.length >= 8 && n.startsWith(k.target)) score = 50 + Math.min(40, k.target.length) / 10 - vi;
      // live photo / motion photo video next to its still: IMG_1.MP4 ↔ IMG_1.HEIC.json
      else if (stemOf(k.target).toLowerCase() === stemOf(n).toLowerCase() && stemOf(n) !== n) score = 20 - vi;
      if (score > bestScore) {
        bestScore = score;
        best = j;
      }
    }
  }
  return best;
}

export interface TakeoutMeta {
  /** capture instant (UTC ms) */
  takenAt: number | null;
  lat: number | null;
  lon: number | null;
}

function geoOf(g: unknown): { lat: number; lon: number } | null {
  if (!g || typeof g !== 'object') return null;
  const lat = Number((g as Record<string, unknown>).latitude);
  const lon = Number((g as Record<string, unknown>).longitude);
  if (!Number.isFinite(lat) || !Number.isFinite(lon) || Math.abs(lat) > 90 || Math.abs(lon) > 180) return null;
  if (Math.abs(lat) < 1e-4 && Math.abs(lon) < 1e-4) return null; // "no location" is written as 0,0
  return { lat, lon };
}

/** fields of a parsed Takeout JSON (geoData = location shown in Google Photos, geoDataExif = camera's) */
export function parseTakeoutJson(j: unknown): TakeoutMeta | null {
  if (!j || typeof j !== 'object') return null;
  const o = j as Record<string, unknown>;
  const ts = (v: unknown) => {
    const n = Number((v as Record<string, unknown> | undefined)?.timestamp);
    return Number.isFinite(n) && n > 0 ? n * 1000 : null;
  };
  const takenAt = ts(o.photoTakenTime) ?? null;
  const geo = geoOf(o.geoData) ?? geoOf(o.geoDataExif);
  if (takenAt === null && !geo) return null;
  return { takenAt, lat: geo ? Math.round(geo.lat * 1e6) / 1e6 : null, lon: geo ? Math.round(geo.lon * 1e6) / 1e6 : null };
}
