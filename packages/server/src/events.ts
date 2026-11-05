import type { ServerEvent } from '@peekaround/shared';

type Listener = (e: ServerEvent) => void;
export type MediaReason = 'add' | 'remove' | 'change' | 'meta' | 'thumb';

/**
 * Minimum spacing between two media events of one folder, per reason.
 * add/remove/change are near-immediate (250 ms coalescing); meta is throttled while scanning;
 * thumb events only patch tiles in place, so they are batched heavily.
 */
export const MEDIA_EVENT_INTERVAL: Record<MediaReason, number> = {
  add: 250,
  remove: 250,
  change: 250,
  meta: 3000,
  thumb: 10_000,
};

interface Pending {
  dirs: Set<string>;
  timer: NodeJS.Timeout | null;
  last: number;
}

/** Tiny pub/sub used by the websocket route; coalesces + rate-limits noisy media events. */
export class EventHub {
  private listeners = new Set<Listener>();
  private pending = new Map<string, Pending>();
  private boot = Date.now().toString(36);
  private counter = 0;
  /** test hook: override intervals */
  intervals: Record<MediaReason, number> = { ...MEDIA_EVENT_INTERVAL };

  on(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  emit(e: ServerEvent) {
    // folders added/removed (cascade deletes) change result sets without a media event
    if (e.type === 'folders') this.counter++;
    for (const l of this.listeners) {
      try {
        l(e);
      } catch {
        /* ignore broken listener */
      }
    }
  }

  /**
   * Generation of the media table for structural / ordering changes (not thumbnails).
   * Combined with per-query count + max(id) into BrowseSkeleton.gen.
   */
  get gen(): string {
    return `${this.boot}.${this.counter}`;
  }

  /** Mark the media table as changed (layout relevant) without emitting an event. */
  bump() {
    this.counter++;
  }

  /** Queue a media event for a folder dir. Events of one reason are coalesced per folder. */
  mediaChanged(folderId: string, dir: string, reason: MediaReason = 'change') {
    if (reason !== 'thumb') this.counter++;
    const key = `${folderId}\u0000${reason}`;
    let p = this.pending.get(key);
    if (!p) this.pending.set(key, (p = { dirs: new Set(), timer: null, last: 0 }));
    p.dirs.add(dir);
    if (p.timer) return;
    const interval = this.intervals[reason];
    const now = Date.now();
    const quick = Math.min(250, interval);
    const delay = Math.max(quick, p.last + interval - now);
    const entry = p;
    entry.timer = setTimeout(() => {
      entry.timer = null;
      entry.last = Date.now();
      const dirs = entry.dirs.has('*') ? ['*'] : [...entry.dirs];
      entry.dirs = new Set();
      this.emit({ type: 'media', folderId, dirs, reason });
    }, delay);
    entry.timer.unref?.();
  }

  /** Drop pending timers (shutdown). */
  close() {
    for (const p of this.pending.values()) if (p.timer) clearTimeout(p.timer);
    this.pending.clear();
  }
}
