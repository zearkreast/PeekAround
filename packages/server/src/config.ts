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
  /** tests: replace the MEGA account API (login / account details / logout) */
  megaAuth?: import('./remote/account.js').MegaAuthClient;
  /** tests: MEGA API gateway of the real clients (a local fake MEGA) */
  megaGateway?: string;
  /** outgoing HTTP for map tiles (desktop: Electron's net.fetch, which honours the system proxy) */
  fetch?: typeof fetch;
}

export interface ResolvedConfig extends Required<Omit<ServerOptions, 'hooks' | 'webDir' | 'roots' | 'mega' | 'megaAuth' | 'megaGateway' | 'security' | 'fetch'>> {
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
    version: opts.version ?? '1.3.2',
    cacheDir,
    thumbDir: path.join(cacheDir, 'thumbs'),
    tempDir: path.join(dataDir, 'temp'),
    fontDir: path.join(dataDir, 'fonts'),
    dbPath: path.join(dataDir, 'peekaround.db'),
  };
}

/**
 * Adapters a phone on the Wi-Fi is never on: Windows' Hyper-V / WSL switches ("vEthernet (WSL)"),
 * VirtualBox / VMware host-only networks, Docker / libvirt bridges, VPN overlays. They often come
 * first on Windows, and the first URL is the one in the QR code.
 */
const VIRTUAL_ADAPTER = /vethernet|hyper-v|\bwsl\b|virtualbox|vbox|vmware|vmnet|docker|^br-|^veth|virbr|tailscale|zerotier/i;

function isPrivateV4(a: string): boolean {
  const [x, y] = a.split('.').map(Number);
  return x === 10 || (x === 172 && y >= 16 && y <= 31) || (x === 192 && y === 168);
}

/**
 * http://<ip>:<port> of this machine for other devices on the network, the most likely one first:
 * private LAN addresses of real adapters, then other addresses, virtual adapters last.
 * Link-local 169.254.x.x (an adapter without a DHCP lease) is left out: nothing can reach it.
 */
export function lanUrls(port: number): string[] {
  const out: { url: string; rank: number; i: number }[] = [];
  for (const [name, list] of Object.entries(os.networkInterfaces())) {
    for (const ni of list ?? []) {
      if (ni.family !== 'IPv4' || ni.internal || ni.address.startsWith('169.254.')) continue;
      const rank = (VIRTUAL_ADAPTER.test(name) ? 2 : 0) + (isPrivateV4(ni.address) ? 0 : 1);
      out.push({ url: `http://${ni.address}:${port}`, rank, i: out.length });
    }
  }
  return out.sort((a, b) => a.rank - b.rank || a.i - b.i).map((o) => o.url);
}
