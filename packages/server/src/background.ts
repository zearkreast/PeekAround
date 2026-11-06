/**
 * One budget for all background work: the scanner's meta / thumbnail / enrichment passes and the
 * duplicate hashing all take a slot here, so however many loops are active, at most `size` items run
 * at once (Settings.indexIntensity, see runtime-settings.ts).
 *
 * - between two items the pool yields (setImmediate, or `gapMs` for the gentle setting), so requests
 *   and IPC (desktop: the server runs in Electron's main process) are never starved by a long pass;
 * - on-demand work (thumbnails / renditions a user is waiting for) calls `foreground()`: new
 *   background items wait until the user has been quiet for a moment, running ones finish normally.
 */
export class BackgroundPool {
  /** items that may run at once */
  size: number;
  /** pause after each item (ms); 0 = just yield to the event loop */
  gapMs: number;
  /** how long background work stays held back after a foreground request (ms) */
  foregroundMs = 1200;
  private active = 0;
  private waiting: (() => void)[] = [];
  private quietUntil = 0;
  private timer: NodeJS.Timeout | null = null;
  private closed = false;

  constructor(size = 2, gapMs = 0) {
    this.size = Math.max(1, Math.floor(size));
    this.gapMs = Math.max(0, gapMs);
  }

  configure(size: number, gapMs = 0) {
    this.size = Math.max(1, Math.floor(size));
    this.gapMs = Math.max(0, gapMs);
    this.pump();
  }

  /** a user is waiting for something: hold new background items back for a moment */
  foreground() {
    if (this.foregroundMs <= 0) return;
    this.quietUntil = Math.max(this.quietUntil, Date.now() + this.foregroundMs);
  }

  get stats() {
    return { active: this.active, waiting: this.waiting.length, size: this.size };
  }

  /** run one background item when a slot is free */
  async run<T>(fn: () => Promise<T> | T): Promise<T> {
    await this.acquire();
    try {
      return await fn();
    } finally {
      this.release();
    }
  }

  /** Run `fn` for every item, at most `size` at once (shared with every other user of the pool). */
  async each<T>(items: T[], fn: (t: T) => Promise<void>, stop: () => boolean = () => false) {
    let i = 0;
    const lanes = Array.from({ length: Math.min(items.length, Math.max(1, this.size)) }, async () => {
      while (i < items.length && !stop()) {
        const it = items[i++];
        await this.run(() => (stop() ? undefined : fn(it)));
      }
    });
    await Promise.all(lanes);
  }

  /** release everything that waits (shutdown); callers check their own stop flag */
  close() {
    this.closed = true;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    for (const w of this.waiting.splice(0)) w();
  }

  private acquire(): Promise<void> {
    if (this.closed) return Promise.resolve();
    return new Promise((resolve) => {
      this.waiting.push(() => {
        this.active++;
        resolve();
      });
      this.pump();
    });
  }

  private release() {
    const done = () => {
      this.active = Math.max(0, this.active - 1);
      this.pump();
    };
    if (this.closed) done();
    else if (this.gapMs > 0) setTimeout(done, this.gapMs);
    else setImmediate(done);
  }

  private pump() {
    if (this.closed || this.timer || !this.waiting.length) return;
    const wait = this.quietUntil - Date.now();
    if (wait > 0) {
      this.timer = setTimeout(() => {
        this.timer = null;
        this.pump();
      }, wait);
      return;
    }
    while (this.active < this.size && this.waiting.length) this.waiting.shift()!();
  }
}
