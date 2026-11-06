import type { Settings } from '@peekaround/shared';
import { DEFAULT_SETTINGS } from '@peekaround/shared';
import type { ResolvedConfig } from './config.js';
import type { Db } from './db.js';
import type { EventHub } from './events.js';
import type { ExploreService } from './explore.js';
import type { Ffmpeg } from './ffmpeg.js';
import type { GeoService } from './geo.js';
import type { LibraryService } from './library.js';
import type { Scanner } from './media/scanner.js';
import type { ThumbService } from './media/thumbs.js';
import type { WatcherService } from './media/watcher.js';
import type { JobService } from './jobs/index.js';
import type { DeviceHub } from './devices/hub.js';
import type { MegaClient } from './remote/mega.js';
import type { RemoteService } from './remote/folders.js';
import { applyRuntimeSettings } from './runtime-settings.js';

export interface Ctx {
  cfg: ResolvedConfig;
  db: Db;
  events: EventHub;
  lib: LibraryService;
  ffmpeg: Ffmpeg;
  thumbs: ThumbService;
  scanner: Scanner;
  watcher: WatcherService;
  jobs: JobService;
  geo: GeoService;
  explore: ExploreService;
  /** linked phones (WS control channel + reply tunnel) */
  devices: DeviceHub;
  /** MEGA access (injectable for tests) */
  mega: MegaClient;
  /** remote (streaming) folders — MEGA links shown without downloading */
  remote: RemoteService;
  lan: { enabled: boolean; port: number };
}

export function getSettings(ctx: Ctx): Settings {
  return { ...DEFAULT_SETTINGS, ...ctx.db.getSetting<Partial<Settings>>('settings', {}) };
}

export function saveSettings(ctx: Ctx, patch: Partial<Settings>): Settings {
  const next = { ...getSettings(ctx), ...patch };
  ctx.db.setSetting('settings', next);
  ctx.scanner.showHidden = next.showHidden;
  if (patch.homeTimeZone) ctx.scanner.setHomeTimeZone(next.homeTimeZone);
  applyRuntimeSettings(ctx, next);
  return next;
}
