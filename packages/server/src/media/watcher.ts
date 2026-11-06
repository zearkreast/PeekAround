import path from 'node:path';
import { watch, type FSWatcher } from 'chokidar';
import type { LibraryService } from '../library.js';
import { toPosix } from '../paths.js';
import type { Scanner } from './scanner.js';

/** Keeps the index in sync with disk changes made outside PeekAround. */
export class WatcherService {
  private watchers = new Map<number, FSWatcher>();

  constructor(
    private lib: LibraryService,
    private scanner: Scanner,
    private poll: boolean,
  ) {}

  sync() {
    const rows = this.lib.allFolderRows().filter((f) => !f.temp && !f.device_id && !f.remote);
    const want = new Set(rows.map((r) => r.id));
    for (const [id, w] of this.watchers) {
      if (!want.has(id)) {
        void w.close();
        this.watchers.delete(id);
      }
    }
    for (const r of rows) if (!this.watchers.has(r.id)) this.start(r.id, r.path);
  }

  private start(folderId: number, root: string) {
    const rel = (p: string) => toPosix(path.relative(root, p));
    const w = watch(root, {
      ignoreInitial: true,
      persistent: true,
      followSymlinks: false,
      usePolling: this.poll,
      interval: 3000,
      binaryInterval: 5000,
      awaitWriteFinish: { stabilityThreshold: 1200, pollInterval: 250 },
      ignored: (p: string) => {
        const r = rel(p);
        if (!r || r.startsWith('..')) return false;
        return this.scanner.skipDir(folderId, r) || r.split('/').some((s) => s.endsWith('.part'));
      },
    });
    w.on('add', (p) => void this.scanner.upsertFile(folderId, rel(p)).catch(() => {}));
    w.on('change', (p) => void this.scanner.upsertFile(folderId, rel(p)).catch(() => {}));
    w.on('unlink', (p) => this.scanner.removeFile(folderId, rel(p)));
    w.on('addDir', (p) => {
      const r = rel(p);
      if (r) this.scanner.events.mediaChanged(String(folderId), path.posix.dirname(r) === '.' ? '' : path.posix.dirname(r));
    });
    w.on('unlinkDir', (p) => {
      const r = rel(p);
      if (r) this.scanner.removeDir(folderId, r);
    });
    w.on('error', () => {
      /* permissions etc. — ignore, rescan still works */
    });
    this.watchers.set(folderId, w);
  }

  async close() {
    await Promise.all([...this.watchers.values()].map((w) => w.close()));
    this.watchers.clear();
  }
}
