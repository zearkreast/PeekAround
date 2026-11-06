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

// ───────────────────────── MEGA account (optional login) ─────────────────────────

/**
 * GET /api/remote/account — the optional MEGA account whose session every MEGA request uses
 * (transfers then count against the account's quota instead of the shared per-IP one).
 * Only the session id is stored on the server; the password never is, and nothing secret is sent here.
 */
export interface MegaAccountStatus {
  loggedIn: boolean;
  email: string | null;
  /** 'free' | 'pro' (Pro I–III, Lite, Flexi) | 'business'; null when MEGA did not say */
  type: 'free' | 'pro' | 'business' | null;
  /** plan name when known, e.g. "Pro I", "Pro Lite" */
  plan: string | null;
  /** transfer quota of the account (bytes) — only when MEGA reports a fixed one */
  transfer: { used: number; total: number | null } | null;
  /** the stored session was rejected by MEGA (expired / logged out elsewhere): log in again */
  expired: boolean;
  /** ms epoch of the login */
  since: number | null;
}

/** POST /api/remote/account/login — `code`: the 6 digit two-factor code when MEGA asks for one */
export interface MegaLoginRequest {
  email: string;
  password: string;
  code?: string;
}

/**
 * login result. Refusals: 'credentials' wrong email or password, 'mfa-required' MEGA wants the two-factor
 * code, 'mfa-invalid' the given code was wrong, 'blocked' the account is blocked by MEGA.
 * Network / rate limit problems are HTTP errors (502 / 429).
 */
export type MegaLoginRefusal = 'credentials' | 'mfa-required' | 'mfa-invalid' | 'blocked';
export type MegaLoginResult = { ok: true; account: MegaAccountStatus } | { ok: false; reason: MegaLoginRefusal };
