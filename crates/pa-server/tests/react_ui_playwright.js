// React web UI served by the Rust server: click through the main pages, collect console errors /
// failed requests, check thumbnails render and a video plays.
const { chromium } = require('playwright');
const base = process.argv[2] || 'http://127.0.0.1:19050';
const shots = process.argv[3] || '.';

(async () => {
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await ctx.newPage();
  const errors = [];
  const failed = [];
  let apiCount = 0;
  page.on('console', (m) => { if (m.type() === 'error') errors.push(`[${page.url()}] ${m.text()}`); });
  page.on('pageerror', (e) => errors.push(`[${page.url()}] pageerror ${e.message}`));
  page.on('response', (r) => {
    if (r.url().includes('/api/')) apiCount++;
    if (r.status() >= 400) failed.push(`${r.status()} ${r.request().method()} ${r.url()}`);
  });
  page.on('requestfailed', (r) => { if (!r.url().includes('/api/tiles/')) failed.push(`FAILED ${r.url()} ${r.failure()?.errorText}`); });

  const report = {};
  const visit = async (name, path, wait = 2500) => {
    await page.goto(base + path, { waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(wait);
    const imgs = await page.$$eval('img', (els) => els.map((e) => ({ src: e.currentSrc || e.src, ok: e.complete && e.naturalWidth > 0 })));
    const thumbs = imgs.filter((i) => i.src.includes('/api/media/'));
    report[name] = { url: page.url(), imgs: imgs.length, thumbs: thumbs.length, thumbsLoaded: thumbs.filter((t) => t.ok).length, title: await page.title() };
    await page.screenshot({ path: `${shots}/${name}.png` });
  };

  await visit('home', '/');
  await visit('library', '/library');
  await visit('library-1', '/library/1');
  await visit('folder', '/folder/1');
  await visit('folder-photos', '/folder/1/' + encodeURIComponent('Fotoğraflar') + '/' + encodeURIComponent('Aile'));
  await visit('albums', '/albums');
  await visit('favorites', '/favorites');
  await visit('search', '/search?q=berlin');
  await visit('explore', '/explore', 4000);
  await visit('settings', '/settings');
  await visit('fonts', '/fonts');

  // open a photo in the viewer
  await page.goto(base + '/folder/1/' + encodeURIComponent('Fotoğraflar') + '/' + encodeURIComponent('Aile'), { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(2500);
  const tile = (await page.$$('img[src*="/api/media/"]'))[1];
  if (tile) {
    await tile.click();
    await page.waitForTimeout(2500);
    const big = await page.$$eval('img', (els) => els.filter((e) => /\/(file|display)/.test(e.currentSrc || e.src)).map((e) => ({ src: e.currentSrc || e.src, ok: e.complete && e.naturalWidth > 0, w: e.naturalWidth })));
    report.viewer = { opened: big.length > 0, images: big };
    await page.screenshot({ path: `${shots}/viewer.png` });
    await page.keyboard.press('ArrowRight');
    await page.waitForTimeout(1200);
    await page.keyboard.press('Escape');
  } else report.viewer = { opened: false, reason: 'no tile' };

  // play a video
  await page.goto(base + '/folder/1/' + encodeURIComponent('Videolar'), { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(2500);
  const vt = await page.$$('img[src*="/api/media/"]');
  report.videoTiles = vt.length;
  const results = [];
  for (let i = 0; i < vt.length; i++) {
    const tiles = await page.$$('img[src*="/api/media/"]');
    if (!tiles[i]) break;
    await tiles[i].click();
    await page.waitForTimeout(3000);
    const v = await page.$('video');
    if (v) {
      await page.evaluate(() => { const v = document.querySelector('video'); if (v) { v.muted = true; v.play().catch(() => {}); } });
      await page.waitForTimeout(2500);
      const st = await page.evaluate(() => { const v = document.querySelector('video'); return v && { src: v.currentSrc, t: v.currentTime, rs: v.readyState, err: v.error && v.error.code, w: v.videoWidth, h: v.videoHeight }; });
      results.push(st);
      await page.screenshot({ path: `${shots}/video-${i}.png` });
    } else results.push({ none: true });
    await page.keyboard.press('Escape');
    await page.waitForTimeout(800);
    await page.goto(base + '/folder/1/' + encodeURIComponent('Videolar'), { waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(1500);
  }
  report.videos = results;
  report.apiRequests = apiCount;
  report.consoleErrors = errors;
  report.failedRequests = failed;
  console.log(JSON.stringify(report, null, 2));
  await browser.close();
})().catch((e) => { console.error(e); process.exit(1); });
