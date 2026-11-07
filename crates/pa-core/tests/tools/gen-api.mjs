// Capture real TypeScript server responses as golden fixtures for pa-core's API types.
// usage: node gen-api.mjs http://127.0.0.1:18901 <outDir>
import fs from 'node:fs';
import path from 'node:path';

const [base, out] = process.argv.slice(2);
fs.mkdirSync(out, { recursive: true });
const saved = [];

async function req(method, url, body) {
  const r = await fetch(base + url, { method, headers: body ? { 'content-type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined });
  const text = await r.text();
  if (!r.ok) throw new Error(`${method} ${url} → ${r.status} ${text.slice(0, 200)}`);
  return JSON.parse(text);
}
/** save a response: `type` = the pa-core type the Rust test deserializes it into */
async function save(name, type, method, url, body) {
  const j = await req(method, url, body);
  fs.writeFileSync(path.join(out, `${name}.json`), JSON.stringify({ type, method, url, body, response: j }, null, 1));
  saved.push(name);
  return j;
}
const get = (name, type, url) => save(name, type, 'GET', url);
const q = (o) => new URLSearchParams(o).toString();

const info = await get('info', 'ServerInfo', '/api/info');
await get('settings', 'Settings', '/api/settings');
const libs = await get('libraries', 'LibrariesResponse', '/api/libraries');
const lib = libs.libraries[0];
const folder = lib.folders[0];

// albums + favorites first so browse / explore show them
const all = await req('GET', `/api/browse?${q({ libraryId: lib.id, recursive: 'true', limit: '500' })}`);
const ids = all.items.map((i) => i.id);
const album = await save('album-create', 'Album', 'POST', '/api/albums', { name: 'Tatil ✈', ids: ids.slice(0, 6) });
await save('album-create-empty', 'Album', 'POST', '/api/albums', { name: 'Boş' });
await req('PATCH', `/api/albums/${album.id}`, { order: 'custom', coverId: ids[3] }).catch(() => {});
await get('albums', 'AlbumsResponse', '/api/albums');
await get('album', 'Album', `/api/albums/${album.id}`).catch(() => {});
await save('album-membership', 'AlbumMembership', 'POST', '/api/albums/membership', { ids: ids.slice(0, 10) });
await req('POST', '/api/media/favorite', { ids: ids.slice(2, 5), favorite: true });
await save('library-patch', 'Library', 'PATCH', `/api/libraries/${lib.id}`, { options: { exclude: ['Thumbnails', '*.tmp'], minBytes: 10 } }).catch(() => {});
await get('libraries-after', 'LibrariesResponse', '/api/libraries');
await get('library-stats', 'LibraryStats', `/api/libraries/${lib.id}/stats`);

// browse
await get('browse-library', 'BrowseResult', `/api/browse?${q({ libraryId: lib.id })}`);
await get('browse-folder', 'BrowseResult', `/api/browse?${q({ folderId: folder.id, dir: '' })}`);
await get('browse-dir', 'BrowseResult', `/api/browse?${q({ folderId: folder.id, dir: 'Fotoğraflar' })}`);
await get('browse-dir2', 'BrowseResult', `/api/browse?${q({ folderId: folder.id, dir: 'Fotoğraflar/2024 Yaz Tatili', sort: 'name', order: 'asc' })}`);
await get('browse-recursive', 'BrowseResult', `/api/browse?${q({ libraryId: lib.id, recursive: 'true', limit: '200' })}`);
await get('browse-videos', 'BrowseResult', `/api/browse?${q({ libraryId: lib.id, recursive: 'true', kind: 'video' })}`);
await get('browse-favorites', 'BrowseResult', `/api/browse?${q({ libraryId: lib.id, recursive: 'true', kind: 'favorites' })}`);
await get('browse-search', 'BrowseResult', `/api/browse?${q({ libraryId: lib.id, recursive: 'true', q: 'istanbul 2024' })}`);
await get('browse-place', 'BrowseResult', `/api/browse?${q({ place: 'TR', recursive: 'true' })}`);
await get('browse-flag', 'BrowseResult', `/api/browse?${q({ flag: 'screenshot', recursive: 'true' })}`);
await get('browse-album', 'BrowseResult', `/api/browse?${q({ albumId: album.id, byPosition: 'true' })}`);
await get('browse-size', 'BrowseResult', `/api/browse?${q({ libraryId: lib.id, recursive: 'true', sort: 'size', order: 'desc', offset: '5', limit: '7' })}`);
await get('skeleton-library', 'BrowseSkeleton', `/api/browse/skeleton?${q({ libraryId: lib.id, recursive: 'true' })}`);
await get('skeleton-dir', 'BrowseSkeleton', `/api/browse/skeleton?${q({ folderId: folder.id, dir: 'Fotoğraflar/Berlin' })}`);
await get('skeleton-name', 'BrowseSkeleton', `/api/browse/skeleton?${q({ libraryId: lib.id, recursive: 'true', sort: 'name' })}`);
await get('timeline', 'TimelineResponse', `/api/timeline?${q({ libraryId: lib.id })}`);

// explore
await get('explore', 'ExploreResult', '/api/explore');
await get('explore-library', 'ExploreResult', `/api/explore?${q({ libraryId: lib.id })}`);
await get('explore-calendar', 'CalendarYear', `/api/explore/calendar?${q({ year: '2024' })}`);
for (let i = 0; i < 20; i++) {
  const d = await req('GET', '/api/explore/duplicates');
  if (!d.pending && d.groups.length) break;
  await new Promise((r) => setTimeout(r, 500));
}
await get('explore-duplicates', 'DuplicatesResponse', '/api/explore/duplicates');
await get('explore-after-dups', 'ExploreResult', '/api/explore');
await get('geo-points', 'GeoPoints', '/api/geo/points');
await get('meta-status', 'MetaStatus', '/api/meta/status');

// media details: every item (images, videos, audio, archive)
for (const it of all.items) await get(`media-${it.id}`, 'MediaDetails', `/api/media/${it.id}`);
for (const it of all.archives ?? []) await get(`media-${it.id}`, 'MediaDetails', `/api/media/${it.id}`);

// jobs, fonts, devices, remote account
const vid = all.items.find((i) => i.kind === 'video');
if (vid) {
  const job = await save('job-video-edit', 'Job', 'POST', '/api/jobs/video-edit', { sourceId: vid.id, trim: { start: 0, end: 1 }, output: { format: 'mp4', height: 120, quality: 'low' } });
  for (let i = 0; i < 60; i++) {
    const j = await req('GET', '/api/jobs');
    if (j.jobs?.every((x) => x.status !== 'running' && x.status !== 'queued')) break;
    await new Promise((r) => setTimeout(r, 500));
  }
  void job;
}
await get('jobs', 'JobsResponse', '/api/jobs');
await get('fonts', 'FontsResponse', '/api/fonts');
await get('devices', 'DevicesResponse', '/api/devices');
await get('remote-account', 'MegaAccountStatus', '/api/remote/account');
const dirRes = await req('GET', `/api/browse?${q({ folderId: folder.id, dir: 'Fotoğraflar' })}`);
const arch = (dirRes.archives ?? [])[0];
if (arch) await save('zip-peek', 'UploadResult', 'POST', '/api/zip/peek', { id: arch.id });
await get('libraries-temp', 'LibrariesResponse', '/api/libraries');
await save('settings-patch', 'Settings', 'PATCH', '/api/settings', { homeTimeZone: 'Europe/Istanbul', mapTiles: 'topo' });
await req('PATCH', '/api/settings', { homeTimeZone: 'UTC', mapTiles: 'osm' });
await get('search', 'SearchResponse', `/api/search?${q({ q: 'kadıköy' })}`).catch(() => {});
console.log(info.version, saved.length, 'responses');
