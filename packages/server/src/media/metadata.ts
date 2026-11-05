import sharp from 'sharp';
import exifr from 'exifr';
import {
  BROWSER_IMAGE_EXT,
  classify,
  isValidGps,
  normalizeCamera,
  offsetFromGps,
  parseDateTimeWithOffset,
  parseGpsDateTime,
  parseIso6709,
  parseOffset,
  resolveFromInstant,
  resolveFromLocal,
  type Playback,
} from '@peekaround/shared';
import { isBrowserPlayable, type Ffmpeg, type ProbeResult } from '../ffmpeg.js';
import type { GeoService } from '../geo.js';
import type { MediaRow } from './mapper.js';

/**
 * Version of the grouping metadata (GPS, camera, flags, time zone handling).
 * Rows with meta_v < META_V are re-read by the background enrichment loop.
 */
export const META_V = 1;

/** time_src column */
export const TIME_SRC = { mtime: 0, local: 1, localOffset: 2, instant: 3 } as const;

export interface ExtractedMeta {
  width: number | null;
  height: number | null;
  duration: number | null;
  playback: Playback;
  extra: Record<string, unknown> | null;
  /** embedded wall clock (EXIF) and its offset, when present */
  local: number | null;
  offset: number | null;
  /** embedded true instant (video creation_time) */
  instant: number | null;
  lat: number | null;
  lon: number | null;
  make: string | null;
  model: string | null;
  lens: string | null;
  gpano: boolean;
  pages: number | null;
}

/** Columns written for the grouping metadata (shared by the meta pass and the enrichment loop). */
export interface GroupColumns {
  taken_at: number;
  taken_local: number;
  tz_off: number | null;
  time_src: number;
  lat: number | null;
  lon: number | null;
  geo_id: number | null;
  cc: string | null;
  admin1: string | null;
  city: string | null;
  make: string | null;
  model: string | null;
  lens: string | null;
  flags: number;
}

const DATE_TAGS = ['DateTimeOriginal', 'CreateDate', 'DateTimeDigitized', 'ModifyDate', 'DateTime'];
const OFFSET_FOR: Record<string, string[]> = {
  DateTimeOriginal: ['OffsetTimeOriginal', 'OffsetTime'],
  CreateDate: ['OffsetTimeDigitized', 'OffsetTime'],
  DateTimeDigitized: ['OffsetTimeDigitized', 'OffsetTime'],
  ModifyDate: ['OffsetTime'],
  DateTime: ['OffsetTime'],
};

const empty = (): ExtractedMeta => ({
  width: null,
  height: null,
  duration: null,
  playback: 'direct',
  extra: null,
  local: null,
  offset: null,
  instant: null,
  lat: null,
  lon: null,
  make: null,
  model: null,
  lens: null,
  gpano: false,
  pages: null,
});

export async function extractMeta(abs: string, row: Pick<MediaRow, 'kind' | 'ext'>, ffmpeg: Ffmpeg): Promise<ExtractedMeta> {
  if (row.kind === 'image') return imageMeta(abs, row.ext);
  if (row.kind === 'video' || row.kind === 'audio') {
    const p = await ffmpeg.probe(abs);
    if (row.kind === 'audio') return { ...empty(), duration: p?.duration ?? null };
    return {
      ...empty(),
      ...videoGroupMeta(p),
      width: p?.width ?? null,
      height: p?.height ?? null,
      duration: p?.duration ?? null,
      playback: isBrowserPlayable(row.ext, p?.codec ?? null) ? 'direct' : 'transcode',
      extra: p
        ? { codec: p.codec, audioCodec: p.audioCodec, fps: p.fps, bitrate: p.bitrate, rotation: p.rotation, hasAudio: p.hasAudio }
        : null,
    };
  }
  return empty();
}

/** GPS / camera / capture time from ffprobe tags (QuickTime, Android, generic). */
export function videoGroupMeta(p: ProbeResult | null): Pick<ExtractedMeta, 'local' | 'offset' | 'instant' | 'lat' | 'lon' | 'make' | 'model'> {
  const t = p?.tags ?? {};
  const loc = parseIso6709(t['com.apple.quicktime.location.iso6709'] ?? t.location ?? t['location-eng']);
  // Apple writes the local time with offset: "2024-07-03T21:00:00+0300"
  const apple = parseDateTimeWithOffset(t['com.apple.quicktime.creationdate']);
  return {
    local: apple && apple.offset !== null ? apple.local : null,
    offset: apple?.offset ?? null,
    instant: p?.creationTime ?? null,
    lat: loc?.lat ?? null,
    lon: loc?.lon ?? null,
    make: t['com.apple.quicktime.make'] ?? t['com.android.manufacturer'] ?? t.make ?? null,
    model: t['com.apple.quicktime.model'] ?? t['com.android.model'] ?? t.model ?? null,
  };
}

async function imageMeta(abs: string, ext: string): Promise<ExtractedMeta> {
  const m = empty();
  m.playback = BROWSER_IMAGE_EXT.includes(ext) ? 'direct' : 'convert';

  if (ext !== 'heic' && ext !== 'heif') {
    try {
      const s = await sharp(abs, { failOn: 'none' }).metadata();
      m.width = s.width ?? null;
      m.height = s.height ?? null;
      // sharp reports the height of ONE page for animated images when pageHeight is set
      if (s.pages && s.pages > 1 && s.pageHeight) m.height = s.pageHeight;
      m.pages = s.pages ?? null;
      if (s.orientation && s.orientation >= 5 && m.width && m.height) [m.width, m.height] = [m.height, m.width];
    } catch {
      /* unreadable → leave unknown */
    }
  }

  if (ext !== 'svg' && ext !== 'gif' && ext !== 'bmp' && ext !== 'ico') {
    try {
      const ex = await exifr.parse(abs, {
        tiff: true,
        exif: true,
        gps: true,
        xmp: true,
        icc: false,
        iptc: false,
        jfif: false,
        ihdr: false,
        interop: false,
        makerNote: false,
        userComment: false,
        reviveValues: false,
        translateValues: false,
        translateKeys: true,
        mergeOutput: true,
      });
      if (ex) applyExif(m, ex);
    } catch {
      /* no exif */
    }
  }
  return m;
}

export function applyExif(m: ExtractedMeta, ex: Record<string, any>) {
  for (const t of DATE_TAGS) {
    const r = parseDateTimeWithOffset(ex[t]);
    if (!r) continue;
    m.local = r.local;
    m.offset = r.offset ?? (OFFSET_FOR[t] ?? []).map((k) => parseOffset(ex[k])).find((o) => o !== null && o !== undefined) ?? null;
    break;
  }
  if (!m.width) {
    const w = Number(ex.ExifImageWidth ?? ex.ImageWidth ?? ex.PixelXDimension) || 0;
    const h = Number(ex.ExifImageHeight ?? ex.ImageHeight ?? ex.PixelYDimension) || 0;
    if (w && h) {
      m.width = w;
      m.height = h;
      if (Number(ex.Orientation) >= 5) [m.width, m.height] = [h, w];
    }
  }
  const lat = Number(ex.latitude);
  const lon = Number(ex.longitude);
  if (isValidGps(lat, lon)) {
    m.lat = Math.round(lat * 1e6) / 1e6;
    m.lon = Math.round(lon * 1e6) / 1e6;
  }
  if (m.local !== null && m.offset === null) {
    const gps = parseGpsDateTime(ex.GPSDateStamp, ex.GPSTimeStamp);
    if (gps !== null) m.offset = offsetFromGps(m.local, gps);
  }
  const str = (v: unknown) => (typeof v === 'string' && v.replace(/\0/g, '').trim() ? v.replace(/\0/g, '').trim().slice(0, 80) : null);
  m.make = str(ex.Make);
  m.model = str(ex.Model);
  m.lens = str(ex.LensModel) ?? str(ex.Lens);
  const proj = String(ex.ProjectionType ?? '').toLowerCase();
  const usePano = ex.UsePanoramaViewer;
  m.gpano = proj === 'equirectangular' || proj === 'cylindrical' || usePano === true || String(usePano).toLowerCase() === 'true';
}

/** Turn extracted metadata into DB columns (time resolution, reverse geocoding, camera, flags). */
export function groupColumns(
  row: Pick<MediaRow, 'name' | 'dir' | 'ext' | 'kind' | 'mtime'>,
  m: ExtractedMeta,
  geo: GeoService,
  homeTimeZone: string,
): GroupColumns {
  let time;
  let src: number;
  if (m.local !== null) {
    time = resolveFromLocal(m.local, m.offset, homeTimeZone);
    src = m.offset !== null ? TIME_SRC.localOffset : TIME_SRC.local;
  } else if (m.instant !== null) {
    time = resolveFromInstant(m.instant, homeTimeZone);
    src = TIME_SRC.instant;
  } else {
    time = resolveFromInstant(row.mtime, homeTimeZone);
    src = TIME_SRC.mtime;
  }
  const cam = normalizeCamera(m.make, m.model);
  const place = isValidGps(m.lat, m.lon) ? geo.nearest(m.lat, m.lon) : null;
  const flags = classify({
    name: row.name,
    dir: row.dir,
    ext: row.ext,
    kind: row.kind,
    width: m.width,
    height: m.height,
    make: m.make,
    model: m.model,
    lens: m.lens,
    gpano: m.gpano,
    pages: m.pages,
  });
  return {
    taken_at: time.takenAt,
    taken_local: time.localAt,
    tz_off: time.tzOffset,
    time_src: src,
    lat: isValidGps(m.lat, m.lon) ? m.lat : null,
    lon: isValidGps(m.lat, m.lon) ? m.lon : null,
    geo_id: place?.id ?? null,
    cc: place?.cc ?? null,
    admin1: place?.admin1Code ?? null,
    city: place?.city || null,
    make: cam.make,
    model: cam.model,
    lens: m.lens,
    flags,
  };
}

export const GROUP_SET_SQL =
  'taken_at = ?, taken_local = ?, tz_off = ?, time_src = ?, lat = ?, lon = ?, geo_id = ?, cc = ?, admin1 = ?, city = ?, make = ?, model = ?, lens = ?, flags = ?';

export function groupParams(g: GroupColumns): (string | number | null)[] {
  return [g.taken_at, g.taken_local, g.tz_off, g.time_src, g.lat, g.lon, g.geo_id, g.cc, g.admin1, g.city, g.make, g.model, g.lens, g.flags];
}

/** Full EXIF for the info panel (on demand). */
export async function readExif(abs: string): Promise<Record<string, string | number | null> | null> {
  try {
    const ex = await exifr.parse(abs, { tiff: true, exif: true, gps: true, xmp: false, icc: false, iptc: false });
    if (!ex) return null;
    const pick: Record<string, string | number | null> = {};
    const keys = [
      'Make',
      'Model',
      'LensModel',
      'FNumber',
      'ExposureTime',
      'ISO',
      'FocalLength',
      'FocalLengthIn35mmFormat',
      'Flash',
      'WhiteBalance',
      'DateTimeOriginal',
      'Software',
      'latitude',
      'longitude',
      'GPSAltitude',
    ];
    for (const k of keys) {
      const v = ex[k];
      if (v === undefined || v === null) continue;
      if (v instanceof Date) pick[k] = v.getTime();
      else if (typeof v === 'number') pick[k] = Math.round(v * 10000) / 10000;
      else pick[k] = String(v).slice(0, 120);
    }
    return Object.keys(pick).length ? pick : null;
  } catch {
    return null;
  }
}
