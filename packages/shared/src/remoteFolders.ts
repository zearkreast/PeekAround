/**
 * Remote (streaming) library folders — a MEGA link shown like a local folder without downloading
 * it (PeekAround 1.2). The link key never leaves the server in these types.
 */
import type { FileRef, FolderRef, Library } from './index';

export type RemoteFolderState = 'listing' | 'ok' | 'offline' | 'quota' | 'error';

/** why a remote folder is not 'ok' */
export type RemoteErrorCode = 'not-found' | 'blocked' | 'key' | 'network' | 'quota' | 'unavailable' | 'other';

export interface RemoteFolderStatus {
  state: RemoteFolderState;
  /** English detail from the server (UI shows a translated text for `code`) */
  error: string | null;
  code: RemoteErrorCode | null;
  /** ms epoch: when the transfer quota resets / the next automatic retry */
  retryAt: number | null;
  /** ms epoch of the last successful listing */
  listedAt: number | null;
}

export interface RemoteFolderInfo extends RemoteFolderStatus {
  provider: 'mega';
  /** the link WITHOUT its key, e.g. "mega.nz/folder/AbCd1234" */
  label: string;
  /** a folder link or a single file link */
  linkKind: 'file' | 'folder';
  /** background metadata + thumbnail progress of this folder's media */
  indexing: { done: number; total: number };
}

/** POST /api/remote/folders */
export interface RemoteFolderCreate {
  url: string;
  /** add to this library; omitted → a new library named `libraryName` (default: the link's name) */
  libraryId?: string;
  libraryName?: string;
  /** folder display name (default: the link's name) */
  name?: string;
}

export interface RemoteFolderCreated {
  library: Library;
  folder: FolderRef;
}

/** POST /api/remote/copy — copy remote media/dirs into a local folder (transfer job) */
export interface RemoteCopySpec {
  ids?: string[];
  refs?: FileRef[];
  to: { folderId: string; dir: string };
}

export const DEFAULT_REMOTE_CACHE_MB = 2048;
