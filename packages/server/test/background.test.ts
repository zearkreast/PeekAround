import { describe, expect, it } from 'vitest';
import { BackgroundPool } from '../src/background';
import { EventHub } from '../src/events';

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

describe('BackgroundPool (one CPU budget for every background pass)', () => {
  it('never runs more than `size` items at once, across several loops', async () => {
    const bg = new BackgroundPool(2);
    let running = 0;
    let peak = 0;
    const item = async () => {
      running++;
      peak = Math.max(peak, running);
      await sleep(5);
      running--;
    };
    const items = Array.from({ length: 12 }, (_, i) => i);
    // e.g. the meta pass, the thumbnail pass and duplicate hashing at the same time
    await Promise.all([bg.each(items, item), bg.each(items, item), Promise.all(items.map(() => bg.run(item)))]);
    expect(peak).toBe(2);
    await sleep(5); // slots are released after a yield
    expect(bg.stats).toMatchObject({ active: 0, waiting: 0 });
  });

  it('can be resized at runtime (Settings.indexIntensity)', async () => {
    const bg = new BackgroundPool(1);
    let running = 0;
    let peak = 0;
    const item = async () => {
      running++;
      peak = Math.max(peak, running);
      await sleep(5);
      running--;
    };
    const all = Promise.all(Array.from({ length: 10 }, () => bg.run(item)));
    bg.configure(3);
    await all;
    expect(peak).toBe(3);
    bg.configure(0); // clamped to 1
    expect(bg.size).toBe(1);
  });

  it('yields to the event loop between items and pauses for gapMs', async () => {
    const bg = new BackgroundPool(1, 30);
    const starts: number[] = [];
    let ticks = 0;
    const t = setInterval(() => ticks++, 1);
    await bg.each([1, 2, 3], async () => {
      starts.push(Date.now());
    });
    clearInterval(t);
    expect(starts[1] - starts[0]).toBeGreaterThanOrEqual(25);
    expect(starts[2] - starts[1]).toBeGreaterThanOrEqual(25);
    expect(ticks).toBeGreaterThan(0);
  });

  it('holds new background items back while a user waits for something (foreground)', async () => {
    const bg = new BackgroundPool(2);
    bg.foregroundMs = 120;
    bg.foreground();
    const t0 = Date.now();
    await bg.run(async () => {});
    expect(Date.now() - t0).toBeGreaterThanOrEqual(100);
    // without foreground activity items start right away
    const t1 = Date.now();
    await sleep(130);
    await bg.run(async () => {});
    expect(Date.now() - t1).toBeLessThan(400);
  });

  it('close() releases waiting loops (shutdown) and later runs do not wait', async () => {
    const bg = new BackgroundPool(1);
    let release!: () => void;
    const busy = bg.run(() => new Promise<void>((r) => (release = r)));
    let second = false;
    const waiting = bg.run(async () => {
      second = true;
    });
    await sleep(10);
    expect(second).toBe(false);
    bg.close();
    await waiting;
    expect(second).toBe(true);
    release();
    await busy;
    await bg.run(async () => {});
  });

  it('each() stops picking items once the stop condition holds', async () => {
    const bg = new BackgroundPool(1);
    const done: number[] = [];
    let stop = false;
    await bg.each(
      [1, 2, 3, 4, 5],
      async (n) => {
        done.push(n);
        if (n === 2) stop = true;
      },
      () => stop,
    );
    expect(done).toEqual([1, 2]);
  });
});

describe('EventHub generation for bulk metadata writes', () => {
  it('moves right away, then at most once per window (leading + trailing edge)', async () => {
    const ev = new EventHub();
    ev.metaGenMs = 80;
    const g0 = ev.gen;
    ev.mediaChanged('1', '', 'meta');
    const g1 = ev.gen;
    expect(g1).not.toBe(g0);
    for (let i = 0; i < 50; i++) ev.mediaChanged('1', '', 'meta');
    // a burst inside the window keeps the generation stable…
    expect(ev.gen).toBe(g1);
    await sleep(120);
    // …and the window's last change is reflected once it ends
    expect(ev.gen).not.toBe(g1);
    const g2 = ev.gen;
    await sleep(120);
    expect(ev.gen).toBe(g2);
    // structural changes still move it immediately
    ev.mediaChanged('1', '', 'add');
    expect(ev.gen).not.toBe(g2);
    ev.close();
  });

  it('status-only folder events leave the generation alone', () => {
    const ev = new EventHub();
    const g = ev.gen;
    ev.emit({ type: 'folders', status: true });
    expect(ev.gen).toBe(g);
    ev.emit({ type: 'folders' });
    expect(ev.gen).not.toBe(g);
    ev.close();
  });
});
