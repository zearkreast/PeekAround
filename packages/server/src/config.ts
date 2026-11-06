import os from 'node:os';
import path from 'node:path';
import type { RunMode } from '@peekaround/shared';

export interface DesktopHooks {
  /** move a file to the OS trash */
  trash?: (absPath: string) => Promise<void>;
}

export interface ServerOptions {
  /** browser security (Host / Origin allow-lists), default from env PEEK_ALLOWED_HOSTS / PEEK_ALLOWED_ORIGINS */
  security?: import('./security.js').SecurityOptions;
  mode?: RunMode;
  host?: string;
  port?: number;
  dataDir: string;
  /** folders that become libraries on first start (one library per folder) */
  roots?: { path: string; name?: string }[];
  /** directory with the built web UI */
  webDir?: string | null;
  ffmpegPath?: string;
  ffprobePath?: string;
  readonly?: boolean;
  watchPoll?: boolean;
  mdns?: boolean;
  /** directories the "add folder" host browser can show (server mode) */
  hostBrowseRoots?: string[];
  maxUploadBytes?: number;
  logger?: boolean;
  hooks?: DesktopHooks;
  version?: string;
  /** tests: replace the MEGA client */
  mega?: import('./remote/mega.js').MegaClient;
  /** outgoing HTTP for map tiles (desktop: Electron's net.fetch, which honours the system proxy) */
  fetch?: typeof fetch;
}

export interface ResolvedConfig extends Required<Omit<ServerOptions, 'hooks' | 'webDir' | 'roots' | 'mega' | 'security' | 'fetch'>> {
  fetch?: typeof fetch;
  hooks: DesktopHooks;
  webDir: string | null;
  roots: { path: string; name?: string }[];
  cacheDir: string;
  thumbDir: string;
  tempDir: string;
  fontDir: string;
  dbPath: string;
}

export function resolveConfig(opts: ServerOptions): ResolvedConfig {
  const dataDir = path.resolve(opts.dataDir);
  const cacheDir = path.join(dataDir, 'cache');
  return {
    mode: opts.mode ?? 'server',
    host: opts.host ?? '0.0.0.0',
    port: opts.port ?? 8080,
    dataDir,
    roots: opts.roots ?? [],
    webDir: opts.webDir ?? null,
    ffmpegPath: opts.ffmpegPath ?? process.env.PEEK_FFMPEG ?? 'ffmpeg',
    ffprobePath: opts.ffprobePath ?? process.env.PEEK_FFPROBE ?? 'ffprobe',
    readonly: opts.readonly ?? false,
    watchPoll: opts.watchPoll ?? false,
    mdns: opts.mdns ?? false,
    hostBrowseRoots: (opts.hostBrowseRoots ?? []).map((p) => path.resolve(p)),
    maxUploadBytes: opts.maxUploadBytes ?? 20 * 1024 * 1024 * 1024,
    logger: opts.logger ?? false,
    hooks: opts.hooks ?? {},
    fetch: opts.fetch,
    version: opts.version ?? '1.2.0',
    cacheDir,
    thumbDir: path.join(cacheDir, 'thumbs'),
    tempDir: path.join(dataDir, 'temp'),
    fontDir: path.join(dataDir, 'fonts'),
    dbPath: path.join(dataDir, 'peekaround.db'),
  };
}

export function lanUrls(port: number): string[] {
  const out: string[] = [];
  for (const list of Object.values(os.networkInterfaces())) {
    for (const ni of list ?? []) {
      if (ni.family === 'IPv4' && !ni.internal) out.push(`http://${ni.address}:${port}`);
    }
  }
  return out;
}
