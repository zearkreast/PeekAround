/**
 * PeekAround shared API contract.
 * Types only (plus a few tiny pure helpers) — imported by the server, the web UI
 * and the Android local backend so every platform speaks the same language.
 */

export const APP_NAME = 'PeekAround';
export const API_VERSION = 1;

// ───────────────────────── Media ─────────────────────────

export type MediaKind = 'image' | 'video' | 'audio' | 'archive';

/** How the client should play/show the original file. */
export type Playback =
  | 'direct' // browser can show/play the original file
  | 'convert' // image the browser can't decode (heic, tiff…) → use display url
  | 'transcode'; // video the browser can't play (mkv/hevc/avi…) → use stream url

export interface MediaItem {
  id: string;
  folderId: string;
  /** Directory relative to the folder root, '' for the root. Always '/' separated. */
  dir: string;
  name: string;
  ext: string; // lower-case, without dot
  kind: MediaKind;
  size: number;
  mtime: number; // ms epoch
  width: number | null;
  height: number | null;
  /** seconds (video/audio) */
  duration: number | null;
  /** ms epoch (true instant) from EXIF / container metadata, falls back to mtime */
  takenAt: number;
  /**
   * Wall-clock capture time encoded as if it were UTC (ms). Use this — with UTC
   * getters / `timeZone: 'UTC'` formatting — for day/month grouping and labels,
   * so a photo taken at 23:30 in Istanbul stays on that day everywhere.
   */
  localAt: number;
  /** MEDIA_FLAG bitmask (screenshot, selfie, panorama, animated) */
  flags: number;
  /** base64 thumbhash for instant blurred placeholder */
  thumbhash: string | null;
  favorite: boolean;
  playback: Playback;
}

export interface MediaDetails extends MediaItem {
  path: string; // absolute path (desktop) or path inside the folder (server)
  exif: Record<string, string | number | null> | null;
  video?: { codec: string | null; fps: number | null; bitrate: number | null; audioCodec: string | null; rotation: number };
  gps: { lat: number; lon: number } | null;
  place: PlaceInfo | null;
  camera: { make: string | null; model: string | null; lens: string | null } | null;
  /** minutes east of UTC when known (EXIF OffsetTime / GPS time), else null */
  tzOffset: number | null;
}

/** Reverse geocoded location (offline GeoNames). */
export interface PlaceInfo {
  /** nearest populated place, e.g. "Kadıköy" */
  city: string;
  /** first-level admin area (province/state), e.g. "İstanbul" */
  admin1: string | null;
  /** ISO 3166-1 alpha-2, e.g. "TR" */
  cc: string;
  /** grouping key used by Places: `${cc}-${admin1 code}` */
  key: string;
}

// ───────────────────────── Libraries & folders ─────────────────────────

export interface FolderRef {
  id: string;
  /** absolute path on the host */
  path: string;
  name: string;
  libraryId: string | null;
  /** temp sessions are zip peeks that disappear on expiry */
  temp: boolean;
  expiresAt: number | null;
  sourceName: string | null;
  mediaCount: number;
  /** set when this folder lives on a linked phone (read + copy only) */
  device: { id: string; name: string; online: boolean } | null;
}

export interface Library {
  id: string;
  name: string;
  color: string; // css color
  icon: string; // lucide icon name
  folders: FolderRef[];
  mediaCount: number;
  sort: number;
}

export interface SubFolder {
  name: string;
  /** dir relative to folder root */
  dir: string;
  mediaCount: number;
  coverIds: string[];
}

export type SortKey = 'takenAt' | 'name' | 'mtime' | 'size' | 'kind';
export type SortDir = 'asc' | 'desc';
export type KindFilter = 'all' | 'image' | 'video' | 'favorites';

export interface BrowseQuery {
  libraryId?: string;
  folderId?: string;
  dir?: string;
  /** include media of all sub folders (flatten) */
  recursive?: boolean;
  sort?: SortKey;
  order?: SortDir;
  kind?: KindFilter;
  q?: string;
  offset?: number;
  limit?: number;
  /** localAt range [from, to) in ms (see MediaItem.localAt) */
  from?: number;
  to?: number;
  /** Places group key (PlaceInfo.key) or a bare country code */
  place?: string;
  /** CameraGroup.key */
  camera?: string;
  flag?: MediaFlagName;
}

export interface Crumb {
  label: string;
  libraryId?: string;
  folderId?: string;
  dir?: string;
}

export interface BrowseResult {
  crumbs: Crumb[];
  folders: SubFolder[];
  /** When browsing a multi-folder library root, its folders are listed here instead. */
  roots: FolderRef[];
  items: MediaItem[];
  total: number;
  /** archives (zip) in this directory that can be peeked */
  archives: MediaItem[];
  /** writable target for drops/uploads at this location, if any */
  target: { folderId: string; dir: string } | null;
  /** same value as BrowseSkeleton.gen for this query (server); a mismatch means the skeleton is stale */
  gen?: string;
}

/**
 * Exact layout skeleton for a browse query: one entry per item in the SAME order
 * as /api/browse pages, so the client can lay out the whole grid before loading items.
 */
export interface BrowseSkeleton {
  /** changes whenever the result set changes; pages carry the same value */
  gen: string;
  total: number;
  kindCounts: { image: number; video: number; audio: number };
  /** base64 of a Uint8Array(total); decode with byteToAspect() */
  aspects: string;
  /**
   * Day groups in order (only when sorted by takenAt): key = 'YYYY-MM-DD' of localAt,
   * count = consecutive items in that day, place = dominant place label or null.
   */
  days: { key: string; count: number; place: string | null }[] | null;
}

/** Quantise an aspect ratio (w/h) into one byte: log scale over [1/8, 8]. */
export function aspectToByte(aspect: number): number {
  const a = Number.isFinite(aspect) && aspect > 0 ? Math.min(8, Math.max(1 / 8, aspect)) : 1;
  return Math.round(((Math.log2(a) + 3) / 6) * 255);
}
export function byteToAspect(b: number): number {
  return 2 ** ((b / 255) * 6 - 3);
}

export interface TimelineBucket {
  key: string; // YYYY-MM
  count: number;
}

// ───────────────────────── Server info & settings ─────────────────────────

export type RunMode = 'server' | 'desktop' | 'android-local';

export interface Capabilities {
  upload: boolean;
  fileOps: boolean;
  /** native folder picker + link folders without copying (desktop) */
  nativeFolders: boolean;
  /** server-side directory browser for adding library folders */
  browseHost: boolean;
  videoJobs: boolean;
  slideshowExport: boolean;
  zipDownload: boolean;
  trash: 'os' | 'folder' | 'none';
  readonly: boolean;
  lanShare: boolean;
}

export interface ServerInfo {
  name: string;
  version: string;
  apiVersion: number;
  mode: RunMode;
  caps: Capabilities;
  hostname: string;
  scanning: boolean;
  lan: { enabled: boolean; urls: string[] };
}

export type ZipAction = 'ask' | 'extract' | 'temp';

export interface Settings {
  zipAction: ZipAction;
  /** IANA zone used to turn UTC-only timestamps (videos, mtime) into local wall-clock time */
  homeTimeZone: string;
  /** map tiles for the Places map */
  mapTiles: 'carto' | 'osm' | 'off';
  editSuffix: string; // default "_edit"
  showHidden: boolean;
  tempSessionHours: number;
  /** server-side only: base directories that the host browser may show */
  hostBrowseRoots: string[];
}

export const DEFAULT_SETTINGS: Settings = {
  zipAction: 'ask',
  homeTimeZone: 'UTC',
  mapTiles: 'osm',
  editSuffix: '_edit',
  showHidden: false,
  tempSessionHours: 24,
  hostBrowseRoots: [],
};

// ───────────────────────── Uploads / zip ─────────────────────────

export type UploadZipMode = 'extract' | 'temp' | 'keep';

export interface UploadResult {
  created: { folderId: string; dir: string; name: string }[];
  /** temp session folder created for a zip in temp mode */
  tempFolder?: FolderRef;
  /** extracted directory (zip extract mode) */
  extractedDir?: string;
}

// ───────────────────────── Fonts ─────────────────────────

export interface FontInfo {
  id: string;
  family: string;
  /** e.g. "Regular", "Bold Italic" */
  subfamily: string;
  weight: number;
  style: 'normal' | 'italic';
  format: 'truetype' | 'opentype' | 'woff' | 'woff2';
  url: string;
  bundled: boolean;
}

// ───────────────────────── Edits & jobs ─────────────────────────

export interface SaveEditResult {
  item: { folderId: string; dir: string; name: string };
}

export interface VideoAdjust {
  brightness: number; // -1..1 (0 = none)
  contrast: number; // -1..1
  saturation: number; // -1..1
  hue: number; // degrees -180..180
  warmth: number; // -1..1
  vignette: number; // 0..1
}

export interface VideoOverlay {
  /** asset id returned by uploadJobAsset (transparent PNG at output resolution) */
  assetId: string;
  start: number; // seconds on the OUTPUT timeline
  end: number;
}

export interface VideoEditSpec {
  sourceId: string;
  trim?: { start: number; end: number };
  /** crop rectangle in source pixels (after rotation is NOT applied) */
  crop?: { x: number; y: number; w: number; h: number };
  rotate?: 0 | 90 | 180 | 270;
  flipH?: boolean;
  flipV?: boolean;
  speed?: number; // 0.25 .. 4
  volume?: number; // 0 .. 2
  mute?: boolean;
  adjust?: Partial<VideoAdjust>;
  overlays?: VideoOverlay[];
  output: {
    format: 'mp4' | 'webm' | 'gif';
    /** target height in px, null = keep */
    height: number | null;
    quality: 'high' | 'medium' | 'low';
  };
}

export type SlideTransition =
  | 'none'
  | 'fade'
  | 'slide'
  | 'zoom'
  | 'kenburns'
  | 'blur'
  | 'flip'
  | 'circle'
  | 'random';

export interface SlideshowExportSpec {
  items: { id: string; duration?: number; trim?: { start: number; end: number } }[];
  /** seconds per image */
  duration: number;
  transition: SlideTransition;
  transitionDuration: number;
  kenBurns: boolean;
  audioId?: string | null;
  width: number;
  height: number;
  fps: number;
  background: string; // hex
  /** where to save: defaults to folder of first item */
  target?: { folderId: string; dir: string };
  name?: string;
}

export type JobStatus = 'queued' | 'running' | 'done' | 'error' | 'canceled';

export interface Job {
  id: string;
  type: 'video-edit' | 'slideshow' | 'transfer';
  title: string;
  status: JobStatus;
  progress: number; // 0..1
  /** transfers: bytes and file counts */
  bytes?: { done: number; total: number };
  files?: { done: number; total: number };
  error: string | null;
  result: { folderId: string; dir: string; name: string } | null;
  createdAt: number;
}

// ───────────────────────── Realtime events ─────────────────────────

export type ServerEvent =
  | { type: 'hello'; info: ServerInfo }
  /**
   * reason: 'add' | 'remove' → structure changed (refetch skeleton);
   * 'meta' → dates/dimensions changed (layout may change); 'thumb' → only
   * thumbhash/thumbnail ready (patch in place, never relayout).
   */
  | { type: 'media'; folderId: string; dirs: string[]; reason?: 'add' | 'remove' | 'change' | 'meta' | 'thumb' }
  | { type: 'devices' } // device list / online state changed
  | { type: 'pair'; device: DeviceInfo } // a phone asks to be paired (desktop window only)
  | { type: 'explore' } // grouping data (places, cameras, memories) changed
  | { type: 'folders' } // libraries / folders / temp sessions changed
  | { type: 'scan'; scanning: boolean; done: number; total: number }
  | { type: 'job'; job: Job }
  | { type: 'fonts' };

// ───────────────────────── File operations ─────────────────────────

export interface FileRef {
  folderId: string;
  /** path relative to folder root (dir + '/' + name, no leading slash) */
  rel: string;
}

export interface HostDirEntry {
  name: string;
  path: string;
}

// ───────────────────────── Explore (automatic grouping) ─────────────────────────

export const MEDIA_FLAG = { screenshot: 1, selfie: 2, panorama: 4, animated: 8 } as const;
export type MediaFlagName = keyof typeof MEDIA_FLAG | 'video';

export interface PlaceGroup {
  key: string; // PlaceInfo.key
  cc: string;
  /** admin1 name, or the city when admin1 is unknown */
  name: string;
  /** most common city inside the group */
  city: string | null;
  count: number;
  coverId: string;
  lat: number;
  lon: number;
}

export interface CameraGroup {
  key: string; // normalised "make|model"
  make: string | null;
  model: string;
  count: number;
  coverId: string;
}

export interface TypeGroup {
  flag: MediaFlagName;
  count: number;
  coverId: string;
}

export interface YearGroup {
  year: number;
  count: number;
  coverId: string;
}

export interface Memory {
  id: string;
  kind: 'onthisday' | 'trip';
  /** onthisday: whole years ago; trip: place label + country */
  yearsAgo?: number;
  place?: string;
  cc?: string;
  /** localAt range [from, to) */
  from: number;
  to: number;
  count: number;
  coverIds: string[];
}

export interface ExploreResult {
  memories: Memory[];
  places: PlaceGroup[];
  types: TypeGroup[];
  cameras: CameraGroup[];
  years: YearGroup[];
  /** background metadata enrichment progress */
  indexing: { done: number; total: number };
}

/** compact map points: [id, lat, lon] */
export interface GeoPoints {
  points: [string, number, number][];
}

// ───────────────────────── Linked devices (phone ↔ PC) ─────────────────────────

export interface DeviceInfo {
  id: string;
  name: string;
  model: string;
  online: boolean;
  /** approved by the PC user */
  paired: boolean;
  lastSeen: number;
  folders: FolderRef[];
  /** pending pairing only: 4-digit code the phone shows too (user compares both screens) */
  code?: string;
  /** what the phone allows: read (list/copy from) and receive (copy into its folders) */
  caps?: { read: boolean; receive: boolean };
}

// ───────────────────────── Remote links (MEGA) ─────────────────────────

export interface RemoteNode {
  id: string;
  /** '/' separated path inside the shared tree, '' for the root */
  path: string;
  name: string;
  size: number;
  dir: boolean;
  kind: MediaKind | null;
}

export interface RemoteInspect {
  provider: 'mega';
  kind: 'file' | 'folder';
  name: string;
  size: number;
  nodes: RemoteNode[];
  truncated: boolean;
}

export interface RemoteImportSpec {
  url: string;
  /** node ids to fetch (files or folders); omitted = everything */
  nodes?: string[];
  /** 'extract' → into folderId/dir; 'temp' → a new temp session */
  mode: 'extract' | 'temp';
  folderId?: string;
  dir?: string;
  /** what to do with zip files inside the link */
  zip?: UploadZipMode;
}

// ───────────────────────── Helpers ─────────────────────────

export const IMAGE_EXT = ['jpg', 'jpeg', 'png', 'gif', 'webp', 'avif', 'bmp', 'svg', 'heic', 'heif', 'tif', 'tiff', 'jfif', 'ico'];
export const BROWSER_IMAGE_EXT = ['jpg', 'jpeg', 'jfif', 'png', 'gif', 'webp', 'avif', 'bmp', 'svg', 'ico'];
export const VIDEO_EXT = ['mp4', 'm4v', 'mov', 'webm', 'mkv', 'avi', 'wmv', 'flv', 'mpg', 'mpeg', '3gp', 'ts', 'mts', 'm2ts', 'ogv'];
export const AUDIO_EXT = ['mp3', 'm4a', 'aac', 'ogg', 'oga', 'opus', 'wav', 'flac'];
export const ARCHIVE_EXT = ['zip'];
export const FONT_EXT = ['ttf', 'otf', 'woff', 'woff2'];

export function extOf(name: string): string {
  const i = name.lastIndexOf('.');
  return i <= 0 ? '' : name.slice(i + 1).toLowerCase();
}

export function kindOf(name: string): MediaKind | null {
  const e = extOf(name);
  if (IMAGE_EXT.includes(e)) return 'image';
  if (VIDEO_EXT.includes(e)) return 'video';
  if (AUDIO_EXT.includes(e)) return 'audio';
  if (ARCHIVE_EXT.includes(e)) return 'archive';
  return null;
}

export function joinRel(dir: string, name: string): string {
  return dir ? `${dir}/${name}` : name;
}

export function parentDir(rel: string): string {
  const i = rel.lastIndexOf('/');
  return i < 0 ? '' : rel.slice(0, i);
}

// ───────────────────────── Pure grouping modules ─────────────────────────

export * from './time';
export * from './geo';
export * from './classify';
export * from './memories';
export * from './trips';
export * from './search';
