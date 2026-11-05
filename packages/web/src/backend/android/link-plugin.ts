import { registerPlugin, type PluginListenerHandle } from '@capacitor/core';

/**
 * Kotlin plugin "PeekLink": share phone folders with a PeekAround PC (docs/link-protocol.md).
 * packages/android/android/app/src/main/java/app/peekaround/PeekLinkPlugin.kt (+ PeekLinkService, LinkClient)
 *
 * A foreground service (notification "PeekAround: N klasör <PC> ile paylaşılıyor" + Stop) keeps one
 * WebSocket to the server while sharing is enabled; it reconnects by itself (backoff, network changes)
 * and survives the app being swiped away. It is only (re)started from the visible UI.
 */

export type LinkState =
  /** sharing disabled / service stopped */
  | 'off'
  /** connecting or waiting for the PC (see `error`: 'no network', 'HTTP 403', 'replaced', …) */
  | 'connecting'
  /** waiting for the PC user to approve; `code` is shown on both screens */
  | 'pending'
  | 'online'
  /** the PC user rejected the phone; sharing was switched off */
  | 'rejected'
  /** fatal: `error` = 'forgotten' (unpaired on the PC; sharing switched off), 'not configured', 'bad url', 'unsupported server version', 'cannot start service' */
  | 'error';

export interface LinkFolderInfo {
  /** stable short id per shared path (protocol `key`) */
  key: string;
  name: string;
  path: string;
}

export interface LinkStatus {
  enabled: boolean;
  state: LinkState;
  /** pairing code while state === 'pending' */
  code?: string;
  /** server base URL, e.g. http://192.168.1.20:47213 */
  server: string | null;
  /** server hostname (from `welcome`) */
  serverName: string | null;
  /** UUID of this phone (send as `X-Peek-Device`) */
  deviceId: string;
  deviceName: string;
  model: string;
  /** pairing token, only once paired — send as `Authorization: Bearer <token>` (+ X-Peek-Device) on API calls to `server` */
  token?: string;
  paired: boolean;
  /** requests the PC is currently being served */
  active: number;
  folders: LinkFolderInfo[];
  error?: string;
}

export interface PeekLinkPlugin {
  status(): Promise<LinkStatus>;
  /**
   * Enable sharing: validates url (http/https only), assigns folder keys (kept per path), stores everything
   * natively, asks for POST_NOTIFICATIONS (Android 13+; refusal only hides the notification) and starts the
   * service. Changing the server URL drops the old token. Must be called from the visible UI.
   */
  start(o: { url: string; folders: { name: string; path: string }[]; name?: string }): Promise<LinkStatus>;
  /** Disable sharing and stop the service (the notification's Stop action does the same). */
  stop(): Promise<LinkStatus>;
  /** Replace the shared folders (max 64); sent to the PC right away when online. Empty list = stop. */
  setFolders(o: { folders: { name: string; path: string }[] }): Promise<LinkStatus>;
  /** Device name shown on the PC (reconnects when running). */
  setName(o: { name: string }): Promise<LinkStatus>;
  /** Drop the pairing token: the next connection asks the PC user again. */
  forget(): Promise<LinkStatus>;
  /** status changes (state / code / error) */
  addListener(event: 'link', fn: (s: LinkStatus) => void): Promise<PluginListenerHandle>;
}

export const PeekLink = registerPlugin<PeekLinkPlugin>('PeekLink');
