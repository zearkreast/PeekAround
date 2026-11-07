# pa-jobs — public API

Port of `packages/server/src/jobs/{index,runners,video-edit,slideshow}.ts` and the `/stream`
transcode of `routes/media.ts`. `ffmpeg-process.ts` (ProgressParser, runFfmpeg, spawnFed,
pipeFrames, listEncoders) lives in **pa-media** (`pa_media::jobs`, `Ffmpeg::encoders`) and is
used from here. Independent of pa-server; everything the server owns is behind traits.

Shared helpers used by pa-link / pa-remote too:
- `pa_jobs::HttpError { status, message, retry_after }` — map to `reply.code(status).send({error: message})`
  (+ `Retry-After` when `retry_after` is set).
- `pa_jobs::EventSink` — the server's EventHub (see below).
- `pa_jobs::paths` — `normalize_rel`, `sanitize_name`, `unique_path`, `unique_edit_path`,
  `resolve_in_root`, `rel_posix`, `set_mtime`, `join_rel` (ports of `paths.ts`).

## Traits S1 implements

```rust
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, ev: pa_core::ServerEvent);                       // events.emit
    fn media_changed(&self, folder_id: &str, dir: &str, reason: MediaReason) { /* default: emit Media */ }
    fn bump(&self) {}                                               // events.bump()
    fn groups_changed(&self) {}                                     // scanner.onGroupsChanged?.()
}

#[async_trait]
pub trait MediaLookup: Send + Sync + 'static {
    fn media(&self, id: &str) -> Result<JobMedia, HttpError>;       // mediaRow (404 'Media not found') + mediaAbs
    fn target_dir(&self, folder_id: &str, dir: &str) -> Result<(i64, PathBuf), HttpError>; // folderPath()
    fn edit_suffix(&self) -> String;                                 // Settings.editSuffix
    async fn upsert_file(&self, folder_id: i64, rel: &str);          // scanner.upsertFile
    async fn render_canvas(&self, m, abs, w, h, bg) -> Result<Vec<u8>, String>; // optional, default = pa-media
}
```
`JobMedia.abs` is `Err(HttpError(403, DEVICE_READONLY | REMOTE_READONLY))` for phone / MEGA rows
(what `mediaAbs` throws) — the runner checks the kind first, then the path, exactly like TS.

## JobService (`ctx.jobs`)

```rust
let jobs = JobService::new(events: Arc<dyn EventSink>, cfg.temp_dir);   // assets in <temp>/job-assets
jobs.enqueue(JobType, title, runner(|jc: JobContext| async move { ...; Ok(Some(FileLocation{..})) })) -> Job
jobs.enqueue_with(JobType, title, |job| job.bytes = Some(..), runner)   // initial fields before the 1st event
jobs.list() -> Vec<Job>          jobs.get(id)           jobs.cancel(id)       jobs.clear_finished()
jobs.wait(id).await -> Option<Job>                      jobs.close().await (shutdown, ≤ 10 s)
jobs.save_asset(&bytes, ext).await / save_asset_stream(body_stream, ext).await -> asset id
jobs.asset_path(id) -> Result<PathBuf, HttpError>       // 400 'Invalid asset id'
```
`JobContext`: `cancel: CancellationToken`, `progress(p)`, `transfer(Some(bytes), Some(files))`,
`set_title`, `set_result` (kept when the runner then fails), `job()`, `is_cancelled()`.
A runner returns `Result<Option<FileLocation>, JobFailure>` (`JobFailure: From<HttpError | io::Error | String | &str | pa_media::jobs::JobError>`);
a runner whose token fired always ends `canceled` with `error: null`, other errors end `error`
with the message cut to 500 UTF-16 units. Lanes: `transfer` jobs run in their own serial lane, the
rest share the `media` lane. Events: `ServerEvent::Job { job }` on every status change and at most
every 300 ms of progress.

## Exporter (`registerJobRunners`)

```rust
let ex = Exporter { jobs, ffmpeg: Arc<pa_media::Ffmpeg>, host: Arc<dyn MediaLookup>, temp_dir };
ex.start_video_edit(&body_json).await -> Result<Job, HttpError>   // 501 no ffmpeg, 400/403/404 validation
ex.start_slideshow(&body_json).await  -> Result<Job, HttpError>
```
Specs are taken as raw `serde_json::Value` (the TS validators coerce strings, accept nulls, and
their 400 messages are part of the API). Pure builders: `video_edit::{normalize_video_edit_spec,
plan_video_edit, build_video_edit_passes, pick_codecs, video_graph, audio_graph, color_filters,
geometry_filters, atempo_chain}`, `slideshow::{normalize_slideshow_spec, plan_slideshow,
piece_range, build_piece_args, build_encoder_args, ken_burns_expr, solid_frame, canvas_size, frame_bytes}`.

## Transcode stream (`GET /api/media/:id/stream?t=`)

```rust
let input = match source { local => stream::file_input(abs),
                           device | remote => stream::internal_input(self_url, internal_token, id) };
let args = stream::transcode_args(&input, stream::parse_t(q.t), settings.hw_accel);
let body = stream::spawn_transcode(&ffmpeg, &args)?;    // impl Stream<Item = io::Result<Bytes>>; drop = SIGKILL
// headers: stream::STREAM_HEADERS (video/mp4, no-store)
```
Route order (TS): `mediaRow` → `kind != video` 400 'Not a video' → `!ffmpeg.available` 501 'ffmpeg not
available' → device: `requireOnline` (503) → input → spawn. `stream::internal_headers(token)` /
`internal_url(self_url, id)` are also the probe options for loopback probes.
`stream::hw_accel_args(bool)` = `hwAccelArgs`. There is no ffmpeg thread cap on jobs/streams in
TS (only `Ffmpeg::set_decode_threads` for thumbnail frames, in pa-media).

## Route mapping

| TS route | call | response |
|---|---|---|
| `GET /api/jobs` | `jobs.list()` | `JobsResponse { jobs }` |
| `DELETE /api/jobs/:id` | `jobs.cancel(id)` | `{ ok: true }` |
| `DELETE /api/jobs` | `jobs.clear_finished(); jobs.list()` | `{ jobs }` |
| `PUT /api/jobs/assets?ext=` (raw body ≤ 256 MB, `requireWritable`) | `jobs.save_asset_stream(body, ext or "png")` | `{ assetId }` |
| `POST /api/jobs/video-edit` (`requireWritable`) | `exporter.start_video_edit(&body)` | `Job` |
| `POST /api/jobs/slideshow` (`requireWritable`) | `exporter.start_slideshow(&body)` | `Job` |
| `GET /api/media/:id/stream?t=` | `stream::{transcode_args, spawn_transcode}` | fragmented MP4 body |
| shutdown | `jobs.close().await` | |
