import type { ServerEvent } from '@peekaround/shared';

type Listener = (e: ServerEvent) => void;

/** Tiny pub/sub used by the websocket route; coalesces noisy media events. */
export class EventHub {
  private listeners = new Set<Listener>();
  private pendingMedia = new Map<string, Set<string>>();
  private flushTimer: NodeJS.Timeout | null = null;

  on(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  emit(e: ServerEvent) {
    for (const l of this.listeners) {
      try {
        l(e);
      } catch {
        /* ignore broken listener */
      }
    }
  }

  /** Coalesce media changes within 250ms into one event per folder. */
  mediaChanged(folderId: string, dir: string) {
    let set = this.pendingMedia.get(folderId);
    if (!set) this.pendingMedia.set(folderId, (set = new Set()));
    set.add(dir);
    if (!this.flushTimer) {
      this.flushTimer = setTimeout(() => {
        this.flushTimer = null;
        const pending = this.pendingMedia;
        this.pendingMedia = new Map();
        for (const [fid, dirs] of pending) this.emit({ type: 'media', folderId: fid, dirs: [...dirs] });
      }, 250);
    }
  }
}
