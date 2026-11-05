import sharp from 'sharp';
import exifr from 'exifr';
import { BROWSER_IMAGE_EXT, type Playback } from '@peekaround/shared';
import { isBrowserPlayable, type Ffmpeg } from '../ffmpeg.js';
import type { MediaRow } from './mapper.js';

export interface ExtractedMeta {
  width: number | null;
  height: number | null;
  duration: number | null;
  takenAt: number | null;
  playback: Playback;
  extra: Record<string, unknown> | null;
}

const DATE_TAGS = ['DateTimeOriginal', 'CreateDate', 'DateTimeDigitized', 'ModifyDate'];

export async function extractMeta(abs: string, row: Pick<MediaRow, 'kind' | 'ext'>, ffmpeg: Ffmpeg): Promise<ExtractedMeta> {
  if (row.kind === 'image') return imageMeta(abs, row.ext);
  if (row.kind === 'video' || row.kind === 'audio') {
    const p = await ffmpeg.probe(abs);
    if (row.kind === 'audio') {
      return { width: null, height: null, duration: p?.duration ?? null, takenAt: null, playback: 'direct', extra: null };
    }
    return {
      width: p?.width ?? null,
      height: p?.height ?? null,
      duration: p?.duration ?? null,
      takenAt: p?.creationTime ?? null,
      playback: isBrowserPlayable(row.ext, p?.codec ?? null) ? 'direct' : 'transcode',
      extra: p
        ? { codec: p.codec, audioCodec: p.audioCodec, fps: p.fps, bitrate: p.bitrate, rotation: p.rotation, hasAudio: p.hasAudio }
        : null,
    };
  }
  return { width: null, height: null, duration: null, takenAt: null, playback: 'direct', extra: null };
}

async function imageMeta(abs: string, ext: string): Promise<ExtractedMeta> {
  let width: number | null = null;
  let height: number | null = null;
  let takenAt: number | null = null;
  const playback: Playback = BROWSER_IMAGE_EXT.includes(ext) ? 'direct' : 'convert';

  if (ext !== 'heic' && ext !== 'heif') {
    try {
      const m = await sharp(abs, { failOn: 'none' }).metadata();
      width = m.width ?? null;
      height = m.height ?? null;
      if (m.orientation && m.orientation >= 5 && width && height) [width, height] = [height, width];
    } catch {
      /* unreadable → leave unknown */
    }
  }

  if (ext !== 'svg' && ext !== 'gif' && ext !== 'bmp' && ext !== 'ico') {
    try {
      const ex = await exifr.parse(abs, {
        pick: [...DATE_TAGS, 'ExifImageWidth', 'ExifImageHeight', 'ImageWidth', 'ImageHeight', 'Orientation'],
        translateValues: false,
      });
      if (ex) {
        for (const t of DATE_TAGS) {
          const v = ex[t];
          if (v instanceof Date && !Number.isNaN(v.getTime()) && v.getFullYear() > 1970) {
            takenAt = v.getTime();
            break;
          }
        }
        if (!width) {
          const w = ex.ExifImageWidth ?? ex.ImageWidth;
          const h = ex.ExifImageHeight ?? ex.ImageHeight;
          if (w && h) {
            width = w;
            height = h;
            if (Number(ex.Orientation) >= 5) [width, height] = [height, width];
          }
        }
      }
    } catch {
      /* no exif */
    }
  }
  return { width, height, duration: null, takenAt, playback, extra: null };
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
