# pa-remote — public API

Port of `packages/server/src/remote/{folders,cache,import}.ts`, the account glue of
`remote/account.ts` and the logic of `routes/remote.ts`, on top of **pa-mega** (MEGA client,
transfer lanes, quota gate, account login). Independent of pa-server: the library DB, scanner,
thumbnail service and import plumbing are behind traits; errors are `pa_jobs::HttpError`
(`status`, `message`, `retry_after` → send `Retry-After` when set). Needs a tokio runtime when
constructed (it spawns its 60 s tick).

## Traits S1 implements

```rust
// pa_remote::store — never called while the service holds a lock (calling back into
// RemoteService::info from folder_ref is fine)
pub trait RemoteStore {
    fn remote_folders(&self) -> Vec<RemoteFolderRow>;          // SELECT * FROM folders WHERE remote IS NOT NULL ORDER BY id
    fn remote_folder(&self, id: i64) -> Result<Option<RemoteFolderRow>, HttpError>; // 404 'Folder not found'; Ok(None) = not a MEGA folder
    fn set_remote_status(&self, id: i64, status_json: &str);   // UPDATE folders SET remote_status = ?
    fn indexing_counts(&self, folder_id: i64) -> (i64, i64);   // (count(*), sum(meta_state = 3 OR thumb_state = 0)) of image/video rows
    fn get_setting(&self, key: &str) -> Option<Value>;  fn set_setting(&self, key: &str, v: Value);   // 'remoteQuota' = {until} | {}
    fn library(&self, id: &str) -> Result<Library, HttpError>;           // lib.library (404)
    fn create_library(&self, name: &str) -> Result<String, HttpError>;   // lib.createLibrary({name, icon:'cloud'}) → id
    fn add_remote_folder(&self, library_id: &str, f: &NewRemoteFolder) -> Result<i64, HttpError>; // lib.addRemoteFolder
    fn folder_ref(&self, folder_id: i64) -> Result<FolderRef, HttpError>; // lib.folderRef — `remote` = RemoteService::info(row)
    fn media_at(&self, folder_path: &str, dir: &str, name: &str) -> Option<RemoteMedia>;
    fn media(&self, id: i64) -> Option<RemoteMedia>;
    fn pending_meta(&self) -> Vec<RemoteMedia>;   // MEGA folders, image/video, meta_state = 3 OR (1 AND meta_v < META_V), ORDER BY taken_local DESC LIMIT 24
    fn apply_meta(&self, row: &RemoteMedia, meta: &ExtractedMeta);  // UPDATE COALESCE(width/height/duration/extra), playback, meta_state 1, meta_v, GROUP_SET_SQL(group_columns(row, meta, geo, homeTz))
    fn meta_failed(&self, id: i64);               // meta_state = 1, meta_v = META_V
}
pub trait ImportHost {                       // remote/import.ts → server
    fn require_writable(&self) -> Result<(), HttpError>;
    fn zip_action(&self) -> String;                                        // Settings.zipAction
    fn target(&self, folder_id: &str, dir: &str) -> Result<(i64, PathBuf, PathBuf), HttpError>; // folderPath(): (id, root, abs dir); 403 REMOTE_READONLY / DEVICE_READONLY
    fn temp_dir(&self) -> PathBuf;                                         // cfg.tempDir
    async fn create_temp_folder(&self, content: &Path, name: &str) -> Result<i64, HttpError>; // lib.createTempFolder(content, name, tempSessionHours)
    async fn delete_temp(&self, folder_id: i64);                           // lib.deleteTemp
    async fn upsert_file(&self, folder_id: i64, rel: &str);                // scanner.upsertFile
    async fn scan_folder(&self, folder_id: i64, rel: &str);                // scanner.scanFolder
    async fn extract_zip(&self, zip: &Path, dest: &Path) -> Result<(), HttpError>;
    async fn zip_to_temp(&self, zip: &Path, name: &str) -> Result<String, HttpError>; // → temp session folder id
}
```
Shared with pa-link (`pa_jobs::library` / `pa_jobs::events`):
- `MediaRowSink` — `skip_file` / `skip_dir` (library exclusions, hidden names) + `list_begin` /
  `list_upsert` / `list_finish` (scope = whole folder). MEGA rows come with `meta_state = 3` for
  images / videos (1 otherwise) and must use the remote variant of the upsert: on conflict
  `width/height/duration/extra = NULL, meta_state = excluded.meta_state, thumb_state = 0, thumbhash = NULL`
  (folders.ts `ingest`); `list_finish` replaces `device_dirs` (empty MEGA dirs show in browse).
- `ThumbCache` — `pending_thumbs(ThumbScope::Remote)`, `set_thumb`, and `generate_base(abs, id)`:
  S1's ThumbService renders `remote://mega/…` paths through `RemoteService::thumb` (its
  RemoteImageSource, background = `Priority::Low`) and uses `is_square_thumb(abs)` like TS.
- `EventSink` — `emit(Folders{status})`, `media_changed`, `groups_changed`, `emit(MegaAccount)`.
- `pa_mega::SessionStore` — settings key `megaSession` (sid + email + since | expired; never the password).

`pa_remote::MegaBackend` (load / read / download / attr / clear_download_urls) is implemented for
`pa_mega::MegaClient`; pass the client from `RemoteAccount` (tests wrap it for fault injection).

## Construction (`app.ts`)

```rust
let acct = pa_remote::RemoteAccount::new(session_store, pa_mega::ApiOptions { gateway, ..Default::default() });
let remote = RemoteService::new(RemoteDeps {
    store, rows, thumbs, events, import, jobs: jobs.clone(), ffmpeg,
    mega: Arc::new(acct.client.clone()),
    cache_dir: cfg.cache_dir.join("remote"),
    cache_bytes: settings.remote_cache_mb as u64 * 1024 * 1024,
});                                   // restores remoteQuota, starts the 60 s tick
acct.bind(&remote);                   // account changed() → clear dl URLs, resume_transfers(), MegaAccount event
remote.set_loopback(&self_url, &devices.internal_token);   // once listening (video probe / frame / transcode input)
remote.start();                       // after scanAll; scanner.remoteScan(id) = remote.refresh(id).await
remote.close().await;                 // shutdown: aborts in-flight MEGA transfers, waits for the loops
```
`acct.account` (`Arc<MegaAccount<MegaAuth>>`) is `ctx.megaAccount`.

## RemoteService calls (TS → Rust)

| TS | Rust |
|---|---|
| `remote.info(row)` (lib.remoteLookup) | `info(&row) -> Result<RemoteFolderInfo>` |
| `remote.create(body)` | `create(&Value) -> RemoteFolderCreated` |
| `remote.refresh(id)` / scanner.remoteScan | `refresh(id)` (shared future, runs even when dropped) |
| `remote.forget(id)` (DELETE /api/folders/:id of a MEGA folder) | `forget(id)` |
| `remote.kick()` (DELETE /api/cache, routes/cache.ts) | `kick()` |
| `remote.cache.setCap / sizeBytes / clear` | `cache.set_cap(bytes)`, `cache.size_bytes()`, `cache.clear()` |
| `remote.resumeTransfers()` | `resume_transfers()` (called by `account_changed`) |
| `remote.serveFile(r, folder, req, reply, download)` | `serve_file(&row, &folder, head, range_header, download) -> FileReply` |
| `remote.details(r, folder)` | S1 builds `MediaDetails` from the row with `exif: None`, `path: details_path(folder.name, dir, name)`, `video: details_video(kind, extra)` |
| `remote.ffmpegInput(r)` (`/stream` of MEGA videos) | `ffmpeg_input(id)` (501 before `set_loopback`) |
| `remote.zipEntry(folder, r)` (`/api/download`) | `zip_entry(&folder, &row) -> BodyStream` (lazy) |
| `remote.copy(spec)` (`/api/fs/move {copy}` from MEGA) | `copy(&refs, &ids, to_folder, to_dir)` or `routes::copy(&svc, host, &body)` |
| RemoteImageSource `thumb / display / strip / isSquareThumb` | `thumb(abs, size, prio)`, `display(abs, max)`, `strip()`, `is_square_thumb(abs)` |
| `remote.quotaUntil` | `quota_until()` / `set_quota_until(ms)` |
| `remote.laneStats(folder)` | `lane_stats(&folder) -> (running, waiting)` |
| `REMOTE_READONLY` (routes/util.ts) | `pa_remote::REMOTE_READONLY` |

## Route mapping (`routes/remote.ts` → `pa_remote::routes`)

| TS route | call | response |
|---|---|---|
| `POST /api/remote/inspect {url}` | `routes::inspect(&svc, &body)` | `pa_mega::RemoteInspect` (serde = TS `RemoteInspect`) |
| `POST /api/remote/import RemoteImportSpec` | `routes::import(&svc, &body)` | `Job` (type transfer) |
| `POST /api/remote/folders RemoteFolderCreate` | `routes::create_folder(&svc, host, &body)` | `RemoteFolderCreated` |
| `POST /api/remote/folders/:id/refresh` | `routes::refresh_folder(&svc, store, id)` | `{ ok: true }` |
| `GET /api/remote/folders/:id/link` | `routes::folder_link(&svc, store, id, mode == desktop, is_lan, ip_is_loopback, has_x_forwarded_for_or_forwarded)` | `{ url }` (403 otherwise) |
| `POST /api/remote/copy RemoteCopySpec` | `routes::copy(&svc, host, &body)` | `Job` |
| `GET /api/remote/account` (requireLocal) | `acct.account.refreshed_status(false).await` | `MegaAccountStatus` |
| `POST /api/remote/account/login` (requireLocal) | `acct.account.login(&body).await` (`pa_mega::LoginRequest`) | `MegaLoginResult` |
| `POST /api/remote/account/logout` (requireLocal) | `acct.account.logout().await` | `MegaAccountStatus` |
| `GET/HEAD /api/media/:id/file` (MEGA row) | `svc.serve_file(..)` | `FileReply` (Accept-Ranges, Cache-Control private 3600, 206 / 416) |
| `GET /api/media/:id/stream` (MEGA row) | `pa_jobs::stream` with `svc.ffmpeg_input(id)` | |
| `GET /api/media/:id/{thumb,display,filmstrip}` (MEGA row) | ThumbService → `thumb / display / strip` | |
| `PATCH /api/settings {remoteCacheMb}` | `svc.cache.set_cap(mb * 1 MiB).await` | |
| `GET /api/cache` | `remoteBytes: svc.cache.size_bytes().await` | |
| `DELETE /api/cache` (core.ts) | `svc.cache.clear().await; svc.kick()` | |
| `DELETE /api/cache/:kind` | thumbs → `svc.kick()` after resetting thumb_state; remote → `svc.cache.clear().await` | |

Route-level only (S1): the rate limits, `requireLocal`, the read-only checks of the fs / upload /
edit routes (`REMOTE_READONLY`), `Retry-After` headers from `HttpError.retry_after`.
