import type {
  Album,
  AlbumOrder,
  LibraryOptions,
  LibraryStats,
  CalendarYear,
  DuplicateGroup,
  MapTileStyle,
  MetaStatus,
  BrowseSkeleton,
  DeviceInfo,
  ExploreResult,
  GeoPoints,
  RemoteImportSpec,
  RemoteInspect,
  RemoteCopySpec,
  RemoteFolderCreate,
  RemoteFolderCreated,
  MegaAccountStatus,
  MegaLoginRequest,
  MegaLoginResult,
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

export interface DuplicatesPage {
  groups: DuplicateGroup[];
  total: number;
  items: MediaItem[];
  /** same-size files still being compared */
  pending: number;
  /** item id → "Folder/sub/dir" */
  paths: Record<string, string>;
}

export type ConnectionState = 'connecting' | 'online' | 'offline';

export type CacheKind = 'thumbs' | 'tiles' | 'remote';
export interface CacheDetails {
  thumbs: { bytes: number };
  tiles: { bytes: number; files: number; limitMb: number };
  remote: { bytes: number; limitMb: number };
}

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
  updateLibrary(id: string, patch: { name?: string; color?: string; icon?: string; sort?: number; coverId?: string | null; options?: Partial<LibraryOptions> }): Promise<Library>;
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
  /** remote (streaming) folders: a MEGA link in a library without downloading (http backends only) */
  remoteAddFolder?(spec: RemoteFolderCreate): Promise<RemoteFolderCreated>;
  remoteRefresh?(folderId: string): Promise<void>;
  /** the link with its key — refused (403) for LAN clients */
  remoteLink?(folderId: string): Promise<string>;
  /** copy remote media / dirs into a local folder (transfer job) */
  remoteCopy?(spec: RemoteCopySpec): Promise<Job>;
  /** bytes in the MEGA stream cache (Settings.remoteCacheMb caps it) */
  remoteCacheSize?(): Promise<number>;
  /** optional MEGA account (its transfer quota instead of the per-IP one) — 403 for LAN clients of the desktop app */
  megaAccount?(): Promise<MegaAccountStatus>;
  megaLogin?(req: MegaLoginRequest): Promise<MegaLoginResult>;
  megaLogout?(): Promise<MegaAccountStatus>;

  cacheSize(): Promise<number>;
  clearCache(): Promise<void>;
  /** cache usage per kind (http backends; settings → performance & cache) */
  cacheDetails?(): Promise<CacheDetails>;
  /** clear one cache kind (refused for LAN clients of the desktop app) */
  clearCacheKind?(kind: CacheKind): Promise<void>;

  // ── maps + Explore insights (optional; a missing method hides its UI) ──
  /** tile URL template through the server ('{z}/{x}/{y}' placeholders); undefined → tiles load from upstream directly */
  tileUrl?(style: MapTileStyle): string;
  tileCache?(): Promise<{ bytes: number; files: number; limitMb: number }>;
  clearTileCache?(): Promise<void>;
  /** items per day of a year (calendar heatmap) */
  calendar?(year: number): Promise<CalendarYear>;
  /** exact duplicate groups (+ their items and folder paths) */
  duplicates?(offset?: number, limit?: number): Promise<DuplicatesPage>;
  /** metadata diagnostics per library */
  metaStatus?(): Promise<MetaStatus>;
  /** re-read metadata (GPS, dates, cameras) of a library — or of everything */
  rereadMeta?(libraryId?: string): Promise<void>;

  // ── libraries (1.2, optional; a missing method hides its UI) ──
  /** header numbers of a library page */
  libraryStats?(id: string): Promise<LibraryStats>;
  /** new library order (ids first → last) */
  orderLibraries?(ids: string[]): Promise<void>;
  /** rescan every folder of a library */
  rescanLibrary?(id: string): Promise<void>;

  // ── albums (1.2, optional: the Android local backend has none → albums are hidden) ──
  albums?(): Promise<Album[]>;
  album?(id: string): Promise<Album>;
  createAlbum?(name: string, ids?: string[]): Promise<Album>;
  updateAlbum?(id: string, patch: { name?: string; coverId?: string | null; order?: AlbumOrder }): Promise<Album>;
  deleteAlbum?(id: string): Promise<void>;
  addToAlbum?(id: string, ids: string[]): Promise<{ added: number; album: Album }>;
  removeFromAlbum?(id: string, ids: string[]): Promise<{ removed: number; album: Album }>;
  /** move ids before `before` (null = end); switches the album to its custom order */
  reorderAlbum?(id: string, ids: string[], before: string | null): Promise<void>;
  orderAlbums?(ids: string[]): Promise<void>;
  /** album id → how many of these items it holds */
  albumMembership?(ids: string[]): Promise<Record<string, number>>;
  albumDownloadUrl?(id: string, name?: string): string;

  subscribe(fn: (e: ServerEvent) => void): () => void;
  onConnection(fn: (s: ConnectionState) => void): () => void;
  dispose(): void;
}
