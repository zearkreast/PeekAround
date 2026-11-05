#!/usr/bin/env node
/**
 * PeekAround mobile/desktop performance harness (Playwright + CDP).
 *
 *   node scripts/perf/run.mjs --label baseline --dist <web dist dir> [--work <scratch dir>] [--lib <dir>] [--data <dir>] [--shots]
 *   (--serve: only start the servers; --mobile-only / --desktop-only; --no-warm; --throttle 4)
 *
 * - starts the API server on :8791 (data + perf library under --work) and `vite preview` on :5181
 *   serving --dist (proxying /api to :8791)
 * - Pixel-class phone emulation (touch, isMobile, DPR 2.625) with 4× CPU throttling:
 *     grid fling scroll (CDP synthesizeScrollGesture, touch) → rAF frame times, long tasks,
 *     tile mounts/renders, backdrop-filter inside the grid
 *     fast scroller jump stability (when present)
 *     viewer: open, swipe ×10 (CDP touch), pinch, double tap, swipe down → frames + requested urls
 * - desktop (1440×900, no throttle): wheel scroll + viewer arrow keys
 * Results are merged into scripts/perf/<label>.json (baseline → baseline.json).
 * Ports: only 8791 / 5181. Servers are killed on exit.
 */
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright');

const argv = process.argv.slice(2);
const arg = (n, d) => {
  const i = argv.indexOf(`--${n}`);
  return i >= 0 ? argv[i + 1] : d;
};
const flag = (n) => argv.includes(`--${n}`);
const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../..');
const WORK = path.resolve(arg('work', process.env.PEEK_PERF_DIR ?? path.join(ROOT, '.perf')));
const LIB = path.resolve(arg('lib', path.join(WORK, 'library')));
const DATA = path.resolve(arg('data', path.join(WORK, 'data')));
const DIST = path.resolve(arg('dist', path.join(ROOT, 'packages/web/dist')));
const LABEL = arg('label', 'run');
const THROTTLE = Number(arg('throttle', 4));
const API = 'http://127.0.0.1:8791';
const WEB = 'http://127.0.0.1:5181';
const OUT = path.join(ROOT, 'scripts/perf', `${LABEL}.json`);
const SHOTS = path.join(ROOT, 'scripts/perf/shots');

const procs = [];
function start(cmd, args, opts) {
  const p = spawn(cmd, args, { stdio: ['ignore', 'pipe', 'pipe'], detached: true, ...opts });
  const log = fs.createWriteStream(path.join(WORK, `${path.basename(args[args.length - 1] ?? cmd)}-${procs.length}.log`));
  p.stdout.pipe(log);
  p.stderr.pipe(log);
  procs.push(p);
  return p;
}
function cleanup() {
  for (const p of procs) {
    try {
      process.kill(-p.pid, 'SIGTERM');
    } catch {
      /* gone */
    }
  }
}
process.on('exit', cleanup);
process.on('SIGINT', () => process.exit(1));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitFor(fn, timeout = 120_000, step = 500) {
  const t0 = Date.now();
  for (;;) {
    try {
      const v = await fn();
      if (v) return v;
    } catch {
      /* not yet */
    }
    if (Date.now() - t0 > timeout) throw new Error('timeout');
    await sleep(step);
  }
}
const getJson = async (u) => (await fetch(u)).json();

function stats(deltas) {
  if (!deltas.length) return null;
  const s = [...deltas].sort((a, b) => a - b);
  const q = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  return {
    frames: s.length,
    p50: +q(0.5).toFixed(1),
    p95: +q(0.95).toFixed(1),
    p99: +q(0.99).toFixed(1),
    max: +s[s.length - 1].toFixed(1),
    over33: s.filter((d) => d > 33.4).length,
    over50: s.filter((d) => d > 50).length,
  };
}

// injected before any page script
const INIT = () => {
  const w = window;
  w.__perf = { frames: [], long: [], tileMounts: 0, rec: false };
  let last = 0;
  const loop = (t) => {
    if (w.__perf.rec && last) w.__perf.frames.push(t - last);
    last = t;
    requestAnimationFrame(loop);
  };
  requestAnimationFrame(loop);
  try {
    new PerformanceObserver((l) => {
      if (w.__perf.rec) for (const e of l.getEntries()) w.__perf.long.push(Math.round(e.duration));
    }).observe({ type: 'longtask', buffered: false });
  } catch {
    /* unsupported */
  }
  const mo = new MutationObserver((muts) => {
    if (!w.__perf.rec) return;
    for (const m of muts)
      for (const n of m.addedNodes) {
        if (n.nodeType !== 1) continue;
        if (n.matches?.('[data-tile]')) w.__perf.tileMounts++;
        w.__perf.tileMounts += n.querySelectorAll?.('[data-tile]').length ?? 0;
      }
  });
  document.addEventListener('DOMContentLoaded', () => mo.observe(document.body, { childList: true, subtree: true }));
  w.__perfStart = () => {
    w.__perf.frames = [];
    w.__perf.long = [];
    w.__perf.tileMounts = 0;
    w.__peekTileRenders = 0;
    last = 0;
    w.__perf.rec = true;
  };
  w.__perfStop = () => {
    w.__perf.rec = false;
    return { frames: w.__perf.frames, long: w.__perf.long, tileMounts: w.__perf.tileMounts, tileRenders: w.__peekTileRenders ?? null };
  };
};

async function measure(page, fn) {
  await page.evaluate(() => window.__perfStart());
  await fn();
  const r = await page.evaluate(() => window.__perfStop());
  return {
    frame: stats(r.frames),
    longTasks: { count: r.long.length, max: r.long.length ? Math.max(...r.long) : 0, over100: r.long.filter((d) => d > 100).length },
    tileMounts: r.tileMounts,
    tileRenders: r.tileRenders,
  };
}

async function backdropInGrid(page) {
  return page.evaluate(() => {
    const els = document.querySelectorAll('[data-grid], [data-grid] *, [data-tile], [data-tile] *');
    let n = 0;
    const seen = new Set();
    for (const el of els) {
      if (seen.has(el)) continue;
      seen.add(el);
      const cs = getComputedStyle(el);
      const bf = cs.backdropFilter || cs.webkitBackdropFilter;
      if (bf && bf !== 'none') n++;
    }
    return { checked: seen.size, withBackdrop: n };
  });
}

async function touchSwipe(cdp, from, to, steps = 8, dt = 16) {
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: from.x, y: from.y, id: 1 }] });
  for (let i = 1; i <= steps; i++) {
    const x = from.x + ((to.x - from.x) * i) / steps;
    const y = from.y + ((to.y - from.y) * i) / steps;
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x, y, id: 1 }] });
    await sleep(dt);
  }
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
}

async function pinch(cdp, c, d0, d1, steps = 10) {
  const pts = (d) => [
    { x: c.x - d / 2, y: c.y, id: 1 },
    { x: c.x + d / 2, y: c.y, id: 2 },
  ];
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: pts(d0) });
  for (let i = 1; i <= steps; i++) {
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: pts(d0 + ((d1 - d0) * i) / steps) });
    await sleep(16);
  }
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
}

async function tap(cdp, x, y) {
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y, id: 1 }] });
  await sleep(40);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
}

function classify(url) {
  const u = new URL(url);
  const m = u.pathname.match(/\/api\/media\/[^/]+\/(\w+)/);
  if (!m) return null;
  const kind = m[1];
  if (kind === 'thumb') return `thumb${u.searchParams.get('s') ?? ''}`;
  if (kind === 'display') return u.searchParams.get('max') ? `display${u.searchParams.get('max')}` : 'display';
  return kind;
}

async function firstVisibleTile(page) {
  return page.evaluate(() => {
    const sc = document.getElementById('scroller');
    const top = sc.getBoundingClientRect().top;
    let best = null;
    for (const el of document.querySelectorAll('[data-tile]')) {
      const r = el.getBoundingClientRect();
      if (r.bottom <= top + 1) continue;
      if (!best || r.top < best.top - 0.5 || (Math.abs(r.top - best.top) < 0.5 && r.left < best.left)) best = { id: el.getAttribute('data-tile'), idx: el.getAttribute('data-index'), top: r.top, left: r.left };
    }
    return best;
  });
}

async function mobile(browser, results) {
  const ctx = await browser.newContext({
    viewport: { width: 412, height: 915 },
    deviceScaleFactor: 2.625,
    isMobile: true,
    hasTouch: true,
    locale: 'tr-TR',
    userAgent: 'Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36',
  });
  await ctx.addInitScript(INIT);
  const page = await ctx.newPage();
  const cdp = await ctx.newCDPSession(page);
  const reqs = [];
  page.on('request', (r) => {
    const c = classify(r.url());
    if (c) reqs.push({ t: Date.now(), c, url: r.url() });
  });
  await page.goto(`${WEB}/timeline`);
  await page.waitForSelector('[data-tile]', { timeout: 60_000 });
  await sleep(2500);
  await cdp.send('Emulation.setCPUThrottlingRate', { rate: THROTTLE });
  await sleep(500);

  // ── grid fling ──
  const grid = await measure(page, async () => {
    // fast finger flicks (CDP touch) → native fling momentum
    for (let i = 0; i < 8; i++) {
      await touchSwipe(cdp, { x: 200, y: 800 }, { x: 205, y: 200 }, 6, 8);
      await sleep(450);
    }
    for (let i = 0; i < 2; i++) {
      await touchSwipe(cdp, { x: 200, y: 200 }, { x: 205, y: 800 }, 6, 8);
      await sleep(450);
    }
    await sleep(1000);
  });
  grid.backdrop = await backdropInGrid(page);
  grid.scrollTop = await page.evaluate(() => document.getElementById('scroller').scrollTop);
  results.mobile.gridFling = grid;
  console.log('mobile grid fling', JSON.stringify(grid));

  // ── fast scroller jump (only when present) ──
  const fs0 = await page.$('[data-fast-scroller]');
  if (fs0) {
    const box = await page.evaluate(() => {
      const el = document.querySelector('[data-fast-scroller]');
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, top: r.top, h: r.height };
    });
    // make the scroller appear, then drag its thumb to ~55%
    await touchSwipe(cdp, { x: 200, y: 600 }, { x: 200, y: 450 }, 5, 16);
    await sleep(150);
    const thumb = await page.evaluate(() => {
      const t = document.querySelector('[data-fast-thumb]');
      if (!t) return null;
      const r = t.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    });
    const jump = { found: !!thumb };
    if (thumb) {
      const t0 = Date.now();
      jump.perf = await measure(page, async () => {
        await touchSwipe(cdp, thumb, { x: thumb.x, y: box.top + box.h * 0.55 }, 12, 20);
        await sleep(30);
      });
      const before = await firstVisibleTile(page);
      await sleep(3000);
      const after = await firstVisibleTile(page);
      jump.before = before;
      jump.after = after;
      jump.sameItem = before && after ? before.idx === after.idx : null;
      jump.drift = before && after ? +Math.abs(after.top - before.top).toFixed(2) : null;
      jump.ms = Date.now() - t0;
    }
    results.mobile.fastScroller = jump;
    console.log('fast scroller', JSON.stringify(jump));
  }

  // ── viewer ──
  await page.evaluate(() => document.getElementById('scroller').scrollTo(0, 0));
  await sleep(1500);
  const tile = await page.evaluate(() => {
    const el = [...document.querySelectorAll('[data-tile]')].find((e) => e.getBoundingClientRect().top > 150);
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  });
  reqs.length = 0;
  const viewerOpen = await measure(page, async () => {
    await tap(cdp, tile.x, tile.y);
    await page.waitForFunction(() => location.search.includes('view='), null, { timeout: 10_000 });
    await sleep(1500);
  });
  const openReqs = reqs.map((r) => r.c);
  reqs.length = 0;
  const ids = new Set();
  const swipes = await measure(page, async () => {
    for (let i = 0; i < 10; i++) {
      await touchSwipe(cdp, { x: 340, y: 480 }, { x: 60, y: 470 }, 8, 16);
      await sleep(650);
      ids.add(new URL(page.url()).searchParams.get('view'));
    }
  });
  const swipeReqs = reqs.map((r) => r.c);
  const countBy = (a) => a.reduce((o, c) => ((o[c] = (o[c] ?? 0) + 1), o), {});
  results.mobile.viewer = { open: viewerOpen, openRequests: countBy(openReqs), swipe: swipes, distinctViews: ids.size, swipeRequests: countBy(swipeReqs) };
  console.log('viewer', JSON.stringify(results.mobile.viewer));

  // gestures: double tap zoom, pinch → zoom rendition, swipe down to close
  reqs.length = 0;
  const g = {};
  const scaleOf = () =>
    page.evaluate(() => {
      const el = document.querySelector('[data-viewer-active] [data-zoom-target]');
      if (!el) return null;
      const m = new DOMMatrix(getComputedStyle(el).transform);
      return +m.a.toFixed(2);
    });
  await tap(cdp, 206, 470);
  await sleep(90);
  await tap(cdp, 206, 470);
  await sleep(900);
  g.doubleTapScale = await scaleOf();
  g.zoomRequestedAfterDoubleTap = reqs.some((r) => r.c.startsWith('display'));
  await tap(cdp, 206, 470);
  await sleep(90);
  await tap(cdp, 206, 470);
  await sleep(900);
  g.afterSecondDoubleTap = await scaleOf();
  reqs.length = 0;
  await pinch(cdp, { x: 206, y: 470 }, 80, 330);
  await sleep(1200);
  g.pinchScale = await scaleOf();
  g.pinchRequests = countBy(reqs.map((r) => r.c));
  await tap(cdp, 206, 470);
  await sleep(90);
  await tap(cdp, 206, 470);
  await sleep(800);
  g.unzoomedScale = await scaleOf();
  results.mobile.gestures = g;
  // swipe down → dismiss
  await touchSwipe(cdp, { x: 206, y: 380 }, { x: 214, y: 760 }, 10, 16);
  await sleep(1200);
  g.closedBySwipeDown = !new URL(page.url()).searchParams.get('view');
  console.log('gestures', JSON.stringify(g));

  // back-stack contract (window.__peekBack)
  results.mobile.backStack = await page.evaluate(() => (window.__peekBack ? window.__peekBack.depth() : null));

  await cdp.send('Emulation.setCPUThrottlingRate', { rate: 1 });
  await ctx.close();
}

async function shots(browser) {
  fs.mkdirSync(SHOTS, { recursive: true });
  const ctx = await browser.newContext({ viewport: { width: 412, height: 915 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true, locale: 'tr-TR' });
  const page = await ctx.newPage();
  const cdp = await ctx.newCDPSession(page);
  await page.goto(`${WEB}/timeline`);
  await page.waitForSelector('[data-tile]');
  await sleep(2500);
  await page.screenshot({ path: path.join(SHOTS, 'mobile-timeline.png') });
  // fast scroller visible
  await touchSwipe(cdp, { x: 200, y: 750 }, { x: 200, y: 250 }, 6, 8);
  await sleep(400);
  await page.screenshot({ path: path.join(SHOTS, 'mobile-fast-scroller.png') });
  await sleep(2500);
  // selection mode via long press
  const t = await page.evaluate(() => {
    const el = [...document.querySelectorAll('[data-tile]')].find((e) => e.getBoundingClientRect().top > 200);
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: t.x, y: t.y, id: 1 }] });
  await sleep(700);
  // drag to range select
  for (let i = 1; i <= 8; i++) {
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x: t.x + i * 4, y: t.y + i * 25, id: 1 }] });
    await sleep(30);
  }
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await sleep(700);
  await page.screenshot({ path: path.join(SHOTS, 'mobile-selection.png') });
  const selected = await page.evaluate(() => document.querySelectorAll('[data-tile][data-selected="1"]').length);
  await page.keyboard.press('Escape');
  await page.goto(`${WEB}/timeline`);
  await page.waitForSelector('[data-tile]');
  await sleep(1500);
  const t2 = await page.evaluate(() => {
    const el = [...document.querySelectorAll('[data-tile]')].find((e) => e.getBoundingClientRect().top > 200);
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  });
  await tap(cdp, t2.x, t2.y);
  await sleep(2000);
  await page.screenshot({ path: path.join(SHOTS, 'mobile-viewer.png') });
  await tap(cdp, 206, 470);
  await sleep(700);
  await page.screenshot({ path: path.join(SHOTS, 'mobile-viewer-immersive.png') });
  await page.goto(`${WEB}/library`);
  await sleep(2000);
  await page.screenshot({ path: path.join(SHOTS, 'mobile-library.png') });
  await page.goto(`${WEB}/timeline`);
  await page.waitForSelector('[data-tile]');
  await sleep(1500);
  await pinch(cdp, { x: 206, y: 500 }, 300, 60, 12);
  await sleep(1500);
  await page.screenshot({ path: path.join(SHOTS, 'mobile-timeline-compact.png') });
  await ctx.close();
  return { rangeSelected: selected };
}

async function desktop(browser, results) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'tr-TR' });
  await ctx.addInitScript(INIT);
  const page = await ctx.newPage();
  const reqs = [];
  page.on('request', (r) => {
    const c = classify(r.url());
    if (c) reqs.push(c);
  });
  await page.goto(`${WEB}/timeline`);
  await page.waitForSelector('[data-tile]', { timeout: 60_000 });
  await sleep(2500);
  await page.mouse.move(700, 500);
  const scroll = await measure(page, async () => {
    for (let i = 0; i < 40; i++) {
      await page.mouse.wheel(0, 500);
      await sleep(40);
    }
    await sleep(800);
  });
  scroll.backdrop = await backdropInGrid(page);
  results.desktop.wheelScroll = scroll;
  console.log('desktop scroll', JSON.stringify(scroll));
  await page.evaluate(() => document.getElementById('scroller').scrollTo(0, 0));
  await sleep(1200);
  await page.click('[data-tile]');
  await page.waitForFunction(() => location.search.includes('view='));
  await sleep(1500);
  reqs.length = 0;
  const nav = await measure(page, async () => {
    for (let i = 0; i < 10; i++) {
      await page.keyboard.press('ArrowRight');
      await sleep(350);
    }
  });
  const countBy = (a) => a.reduce((o, c) => ((o[c] = (o[c] ?? 0) + 1), o), {});
  results.desktop.viewerArrows = { ...nav, requests: countBy(reqs) };
  console.log('desktop viewer', JSON.stringify(results.desktop.viewerArrows));
  await ctx.close();
}

async function main() {
  fs.mkdirSync(WORK, { recursive: true });
  if (!fs.existsSync(LIB)) throw new Error(`perf library missing: ${LIB} (run scripts/perf/gen-library.mjs first)`);
  if (!fs.existsSync(path.join(DIST, 'index.html'))) throw new Error(`no web build in ${DIST}`);
  start('npx', ['tsx', 'packages/server/src/cli.ts', '--data', DATA, '--root', LIB, '--port', '8791', '--no-mdns'], { cwd: ROOT, env: { ...process.env, PEEK_PORT: '8791' } });
  await waitFor(() => getJson(`${API}/api/info`), 90_000);
  const total = await waitFor(async () => {
    const info = await getJson(`${API}/api/info`);
    const b = await getJson(`${API}/api/browse?recursive=1&limit=1`);
    return !info.scanning && b.total > 0 ? b.total : 0;
  }, 600_000, 2000);
  console.log(`server ready: ${total} items`);
  // warm thumbnails so baseline and after runs see the same server cache
  if (!flag('no-warm')) {
    const b = await getJson(`${API}/api/browse?recursive=1&limit=5000&sort=takenAt&order=desc`);
    const ids = b.items.map((i) => i.id);
    let k = 0;
    await Promise.all(
      Array.from({ length: 8 }, async () => {
        while (k < ids.length) {
          const id = ids[k++];
          await fetch(`${API}/api/media/${id}/thumb?s=320`).then((r) => r.arrayBuffer()).catch(() => {});
        }
      }),
    );
    for (const id of ids.slice(0, 40)) {
      await fetch(`${API}/api/media/${id}/thumb?s=640`).then((r) => r.arrayBuffer()).catch(() => {});
      await fetch(`${API}/api/media/${id}/thumb?s=1600`).then((r) => r.arrayBuffer()).catch(() => {});
    }
    console.log('thumbnails warm');
  }
  start('npx', ['vite', 'preview', '--port', '5181', '--strictPort', '--outDir', DIST], { cwd: path.join(ROOT, 'packages/web'), env: { ...process.env, PEEK_API: API } });
  await waitFor(() => fetch(WEB).then((r) => r.ok), 60_000);

  if (flag('serve')) {
    console.log(`serving: ${WEB} (api ${API}) — Ctrl+C to stop`);
    await new Promise(() => {});
  }
  const browser = await chromium.launch({ args: ['--enable-features=OverlayScrollbar'] });
  const results = { label: LABEL, date: new Date().toISOString(), items: total, throttle: THROTTLE, mobile: {}, desktop: {} };
  try {
    if (!flag('desktop-only')) await mobile(browser, results);
    if (!flag('mobile-only')) await desktop(browser, results);
    if (flag('shots')) results.shots = await shots(browser);
  } finally {
    await browser.close();
  }
  fs.writeFileSync(OUT, JSON.stringify(results, null, 2));
  console.log(`saved ${OUT}`);
  cleanup();
  process.exit(0);
}

main().catch((e) => {
  console.error(e);
  cleanup();
  process.exit(1);
});
