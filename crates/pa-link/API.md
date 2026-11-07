# pa-link — public API

Port of `packages/server/src/devices/{hub,access,media,transfer}.ts` + `routes/devices.ts`
(protocol: docs/link-protocol.md, wire-compatible with the 1.3.x Android LinkClient — frames,
headers, close codes and status codes are unchanged). Transport-agnostic core; feature `axum`
(default) adds ready-made glue in `pa_link::axum`.

## Traits S1 implements

```rust
// pa_link::store
pub trait DeviceStore {            // the `devices` table + device folder rows
    fn device(&self, id) -> Option<DeviceRow>;
    fn paired_devices(&self) -> Vec<DeviceRow>;                 // paired = 1 ORDER BY last_seen DESC
    fn save_paired(&self, id, name, model, token_hash, caps_json, now);   // approve (upsert)
    fn update_online(&self, id, name, model, caps_json, now);
    fn set_last_seen(&self, id, now);
    fn delete_device(&self, id);                                 // + its folders, media rows, device_dirs
    fn device_folders(&self, device_id) -> Vec<DeviceFolderRow>; // ORDER BY sort, id
    fn all_device_folders(&self) -> Vec<(i64, String)>;          // (folder id, device id)
    fn device_folder(&self, folder_id) -> Option<DeviceFolderRow>;
    fn sync_device_folders(&self, device_id, update: &[(id, name, sort)], insert: &[(key, name, sort)], delete: &[id]) -> Vec<i64>;
                                                                 // one tx; inserts use path device://<id>/<key>, library_id NULL
    fn folder_refs(&self, device_id) -> Vec<FolderRef>;          // lib.folderRef(row, counts) — may call hub.device_lookup()
}
// pa_jobs::library (shared with pa-remote)
pub trait MediaRowSink { skip_name, skip_file, skip_dir, list_begin, list_upsert, list_abort, list_finish }
pub trait ThumbCache   { async generate_base(abs, media_id), set_thumb(..), pending_thumbs(ThumbScope::Device(id) | Remote) }
pub trait EventSink    { emit, media_changed, bump, groups_changed }
// pa_link::transfer
pub trait TransferHost { folder(id), device_folder(id), find_media(f, dir, name), media_under(f, rel), skip_name, async upsert_file(f, rel) }
```
`ListedFile { dir, name, ext, kind, size, mtime, meta: pa_meta::ExtractedMeta }` — compute the
grouping columns with `pa_meta::group_columns(&RowInfo{..}, &meta, geo, home_tz)` and run the
`upsertListed` SQL (meta_state 1, thumb_state 0, seen 1). `list_upsert` returns inserted+changed.
**Locking rule:** the hub never calls a trait while holding its lock, but `EventSink::emit` and the
store must not block on the hub either (calling `hub.device_lookup()` from `folder_refs` is fine).

## DeviceHub (`ctx.devices`)

```rust
let hub = DeviceHub::new(HubDeps { store, rows, thumbs, events, server_name: hostname });  // starts the 25 s heartbeat
hub.set_self_url("http://127.0.0.1:<port>");   // once listening (loopback ffmpeg inputs)
hub.internal_token                              // X-Peek-Internal value
```
| call | TS |
|---|---|
| `attach(ConnectMeta{lan, header_id, token}) -> (SocketHandle, Receiver<WsOut>)` | `attach(socket, req)`; feed `on_text / on_binary / on_pong / on_close`, send `WsOut::{Text, Ping, Close(code, reason), Terminate}` |
| `authenticate(x_peek_device, authorization) -> Result<device_id>` / `paired_device` | same (401 / 403) |
| `list(hide: Option<&str>) -> Vec<DeviceInfo>` | `list(hide)` |
| `approve(id)`, `reject(id)`, `forget(id)` | same (404s) |
| `request(device, op, args, ReqOpts{prio, signal, first_byte}) -> TunnelReply` | `request` (drop the future = abort; `TunnelReply.body` is a `Stream`, drop = cancel + 499) |
| `receive(device, args, opts) -> Value` | `request(…'receive'…)` |
| `accept_reply(rid, device, headers, body_stream) -> 204/410/499` | `acceptReply` |
| `fetch_buffer`, `thumb(abs, size, high)`, `display(abs, max)`, `strip(abs, n, h)` | RemoteImageSource for ThumbService |
| `relist(folder_id, sub_dir)` (shared future) | `relist` / `scanner.deviceScan` |
| `details(device, key, rel) -> Option<DeviceDetails>` | `details` |
| `create_pull / take_pull / revoke_pull / hold_receive / await_receive_result / rid_of_pull` | same |
| `is_online, has_folders, folder_owner, caps, stats, device_lookup, parse_abs` | same |
| `close_lan()` | `setLan(false)` closing LAN phone sockets (1001) |
| `close().await` | shutdown |
`hub.timeouts.{short_ms, file_ms}` (AtomicU64) = `FIRST_BYTE_MS`.

## Media helpers (`pa_link::media`)
- `serve_device_file(hub, device_id, key, &FileRow{dir,name,ext,size}, head, range_header, download) -> FileReply`
  (`pa_jobs::library::FileReply { status, headers, body: Option<BodyStream> }`).
- `require_online`, `device_ffmpeg_input(hub, media_id)` (501 before `set_self_url`),
  `lazy_device_stream(hub, device, key, rel)` (zip entries).
- `deviceDetails`: `d = hub.details(..)`; `m = refine_meta(&d, base_meta)` → group columns + UPDATE
  (events `media_changed(.., Meta)` + `groups_changed` when taken_local/lat/lon/make/model/flags changed);
  response `exif: details_exif(d.as_ref())`, `path: details_path(device_name, folder_name, rel)`.

## Access (`pa_link::access`)
`RequestInfo { headers (lower-case), query, lan }`, `RunMode::{Desktop, Server}`:
`viewer_of(hub, mode, req) -> Viewer{devices, self_id}`, `require_approver(mode, req, Some(id))`,
`can_see_pairing(mode, req)` (also: who gets `pair` events), `device_auth_of(req)` (`?pdev=&ptok=`),
`referenced(query, route_pattern, id_param, body)` + `check_access(hub, mode, url, req, &refs, folder_of_media)`
= `deviceAccessHook` (404 'Folder not found' / 'Media not found').

## Transfers (`pa_link::transfer::Transfers { hub, jobs, host }`)
- `copy_from_device(&refs, &to: HostFolder, to_dir) -> Result<Job>` (route checked `to` is local/writable)
- `copy_to_device(&refs, &DeviceFolderRow, to_dir).await -> Result<Job>`
- `open_pull(hub, token, device_id) -> FileReply`

## Route mapping

| TS route | call |
|---|---|
| `GET /api/devices` | `v = viewer_of(..)`; `!v.devices → []`; `all = hub.list(v.self_id)`; `can_see_pairing ? all : paired only` |
| `POST /api/devices/:id/approve` | `require_approver(mode, req, Some(id))?; hub.approve(id)?` → `{ok:true}` |
| `POST /api/devices/:id/reject` | `require_approver(mode, req, Some(id))?; hub.reject(id)?` |
| `DELETE /api/devices/:id` | `require_approver(mode, req, None)?; hub.forget(id)?` |
| `GET /api/devices/connect` (WS) | `pa_link::axum::serve_socket(hub, ws, connect_meta(&headers, lan))` |
| `PUT /api/devices/reply/:rid` (raw body, no parsing) | `pa_link::axum::reply_put(&hub, rid, &headers, body)` |
| `GET /api/devices/pull/:token` | `pa_link::axum::pull_get(&hub, token, &headers)` |
| `GET/HEAD /api/media/:id/file` (device) | `serve_device_file(..)` → `axum::file_response` |
| `GET /api/media/:id/stream` (device) | `require_online` + `pa_jobs::stream` with `device_ffmpeg_input` |
| `GET /api/media/:id` (device) | `details` flow above |
| `/preview` (device) | redirect to `/file` (S1) |
| `/thumb`, `/display`, `/filmstrip` (device) | ThumbService with `hub.thumb/display/strip` as its remote source |
| `POST /api/fs/move {copy:true}` | `transfers.copy_from_device` / `copy_to_device` → `{ moved: [], job }` |
| `/api/download` zip entries (device) | `lazy_device_stream` |
| every `/api/` preHandler | `check_access(..)` |

Notes: an early 499 on a reply PUT is sent with `Connection: close`; HTTP clients that are still
uploading may see it as a reset (the Android app treats both the same). A `serde_json` with the
`float_roundtrip` feature is recommended for JS-exact number parsing (pa-jobs golden tests use it).
