import type { MediaItem, MediaKind, Playback } from '@peekaround/shared';
import type { Row } from '../db.js';

export interface MediaRow {
  id: number;
  folder_id: number;
  dir: string;
  name: string;
  ext: string;
  kind: MediaKind;
  size: number;
  mtime: number;
  width: number | null;
  height: number | null;
  duration: number | null;
  taken_at: number;
  thumbhash: string | null;
  favorite: number;
  playback: Playback;
  meta_state: number;
  thumb_state: number;
  extra: string | null;
  // grouping metadata (migration 001)
  taken_local: number | null;
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
  meta_v: number;
}

export function toItem(r: MediaRow | Row): MediaItem {
  return {
    id: String(r.id),
    folderId: String(r.folder_id),
    dir: r.dir,
    name: r.name,
    ext: r.ext,
    kind: r.kind,
    size: r.size,
    mtime: r.mtime,
    width: r.width,
    height: r.height,
    duration: r.duration,
    takenAt: r.taken_at || r.mtime,
    // wall clock at the place of capture, encoded as UTC (see MediaItem.localAt)
    localAt: r.taken_local ?? (r.taken_at || r.mtime),
    flags: r.flags ?? 0,
    thumbhash: r.thumbhash,
    favorite: !!r.favorite,
    playback: r.playback,
  };
}

export const ITEM_COLS =
  'id, folder_id, dir, name, ext, kind, size, mtime, width, height, duration, taken_at, taken_local, flags, thumbhash, favorite, playback';
