import type {
  BrowseSkeleton,
  DeviceInfo,
  ExploreResult,
  GeoPoints,
  RemoteImportSpec,
  RemoteInspect,
  BrowseQuery,
  BrowseResult,
  FileRef,
  FolderRef,
  FontInfo,
  HostDirEntry,
  Job,
  Library,
  MediaDetails,
  MediaItem,
  ServerEvent,
  ServerInfo,
  Settings,
  SlideshowExportSpec,
  TimelineBucket,
  UploadResult,
  UploadZipMode,
  VideoEditSpec,
} from '@peekaround/shared';

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

export interface UploadOptions {
  folderId: string;
  dir: string;
  /** relative path incl. file name, e.g. "Holiday/IMG_1.jpg" */
  path: string;
  zip?: UploadZipMode;
  /** original modification time (ms) to preserve on the server */
  mtime?: number;
  onProgress?: (loaded: number, total: number) => void;
  signal?: AbortSignal;
}

export interface SaveEditOptions {
  /** save as a copy next to this media item */
  sourceId?: string;
  /** …or as a new file in this folder */
  folderId?: string;
  dir?: string;
  name?: string;
  format: 'jpg' | 'png' | 'webp';
  quality?: number;
}

export type ConnectionState = 'connecting' | 'online' | 'offline';

/**
 * Everything the UI needs from "where the media lives".
 * HttpBackend talks to a PeekAround server (web, Docker, desktop, Android client mode);
 * the Android local backend talks to the device storage through a Capacitor plugin.
 */
export interface Backend {
  readonly id: string;
  readonly kind: 'http' | 'android-local';
  readonly label: string;

  info(): Promise<ServerInfo>;
  settings(): Promise<Settings>;
  updateSettings(patch: Partial<Settings>): Promise<Settings>;

  /** devices: linked phones (http backends only; desktop window / paired devices) */
  libraries(): Promise<{ libraries: Library[]; temp: FolderRef[]; devices?: DeviceInfo[] }>;
  createLibrary(input: { name: string; color?: string; icon?: string; folders?: string[] }): Promise<Library>;
  updateLibrary(id: string, patch: { name?: string; color?: string; icon?: string; sort?: number }): Promise<Library>;
  deleteLibrary(id: string): Promise<void>;
  addFolder(libraryId: string, path: string, name?: string): Promise<FolderRef>;
  renameFolder(folderId: string, name: string): Promise<void>;
  removeFolder(folderId: string): Promise<void>;
  rescanFolder(folderId: string): Promise<void>;
  hostDirs(path?: string): Promise<{ path: string | null; parent: string | null; dirs: HostDirEntry[] }>;
  hostMkdir(path: string, name: string): Promise<{ path: string }>;

  browse(q: BrowseQuery, opts?: { signal?: AbortSignal }): Promise<BrowseResult>;
  timeline(libraryId?: string): Promise<{ buckets: TimelineBucket[] }>;
  /** exact layout skeleton for a browse query (same order as browse pages) */
  skeleton(q: BrowseQuery): Promise<BrowseSkeleton>;
  /** automatic groups: memories, places, types, cameras, years */
  explore(): Promise<ExploreResult>;
  geoPoints(q: BrowseQuery): Promise<GeoPoints>;
  /** url of a bounded rendition (longest side ≤ 4096) for zooming; never the raw original */
  zoomUrl(item: Pick<MediaItem, 'id' | 'playback' | 'kind'>): string;
  media(id: string): Promise<MediaDetails>;
  setFavorite(ids: string[], favorite: boolean): Promise<void>;

  thumbUrl(id: string, size: number): string;
  fileUrl(id: string, download?: boolean): string;
  /** url to show an image at full size (converted when the browser can't decode the original) */
  displayUrl(item: Pick<MediaItem, 'id' | 'playback' | 'kind'>): string;
  streamUrl(id: string, startAt?: number): string;
  previewUrl(id: string): string;
  filmstripUrl(id: string, n: number, h: number): string;
  downloadUrl(ids: string[], name?: string): string;
  folderDownloadUrl(folderId: string, dir: string, name?: string): string;
  /** optional: hand files to the OS share sheet instead of downloading (phone) */
  share?(ids: string[]): Promise<void>;

  upload(file: Blob, opts: UploadOptions): Promise<UploadResult>;
  importLocal(paths: string[], folderId: string, dir: string, zip?: UploadZipMode): Promise<UploadResult>;
  peekZip(id: string): Promise<{ tempFolder: FolderRef }>;
  extractZip(id: string): Promise<{ folderId: string; dir: string }>;
  keepTemp(tempId: string, target: { folderId: string; dir: string }): Promise<{ folderId: string; dir: string }>;
  extendTemp(tempId: string, hours: number): Promise<void>;

  mkdir(folderId: string, dir: string, name: string): Promise<{ folderId: string; dir: string }>;
  rename(ref: FileRef, name: string): Promise<{ rel: string }>;
  /** copies from / into a linked phone's folder (http) resolve with the transfer job */
  move(refs: FileRef[], to: { folderId: string; dir: string }, copy?: boolean): Promise<void | { moved?: string[]; job?: Job }>;
  remove(refs: FileRef[]): Promise<void>;

  fonts(): Promise<FontInfo[]>;
  uploadFont(file: File): Promise<FontInfo[]>;
  deleteFont(id: string): Promise<FontInfo[]>;
  fontUrl(font: FontInfo): string;

  saveEdit(image: Blob, opts: SaveEditOptions): Promise<{ item: { folderId: string; dir: string; name: string }; id: string | null }>;

  jobs(): Promise<Job[]>;
  cancelJob(id: string): Promise<void>;
  clearJobs(): Promise<void>;
  uploadJobAsset(blob: Blob, ext?: string): Promise<string>;
  startVideoEdit(spec: VideoEditSpec): Promise<Job>;
  startSlideshow(spec: SlideshowExportSpec): Promise<Job>;

  // ── linked phones (http backends; undefined elsewhere) ──
  devices?(): Promise<DeviceInfo[]>;
  /** approve / reject a pairing request (desktop window only) */
  pairDevice?(id: string, approve: boolean): Promise<void>;
  forgetDevice?(id: string): Promise<void>;

  // ── remote links (MEGA) ──
  remoteInspect?(url: string): Promise<RemoteInspect>;
  remoteImport?(spec: RemoteImportSpec): Promise<Job>;

  cacheSize(): Promise<number>;
  clearCache(): Promise<void>;

  subscribe(fn: (e: ServerEvent) => void): () => void;
  onConnection(fn: (s: ConnectionState) => void): () => void;
  dispose(): void;
}
