# PeekAround Link — phone ↔ PC protocol (v1)

The phone (Android app, native foreground service `PeekLinkService`) keeps ONE WebSocket
open to a PeekAround server (desktop app with LAN sharing, or Docker). The PC never
connects to the phone: every byte flows over phone-initiated connections, so it works
whenever the phone can reach the PC.

* Control plane: WebSocket `GET /api/devices/connect` — small JSON text frames only (< 64 KB).
* Data plane: for every request that returns data, the phone opens
  `PUT /api/devices/reply/:rid` and streams the body. The server pipes it into the
  waiting consumer (HTTP response to a browser, a copy job, the thumbnail cache…).
* PC → phone file pushes: the phone `GET`s a one-time pull URL.

All phone→server HTTP requests carry `Authorization: Bearer <deviceToken>` and
`X-Peek-Device: <deviceId>`. Unknown/invalid token → 401. Not yet paired → 403 (except connect).

## 1. Connect & pairing

```
GET /api/devices/connect?v=1          (WebSocket upgrade)
Authorization: Bearer <token>          (absent on the very first connection)
X-Peek-Device: <deviceId>              (UUID generated once by the phone, stored in SharedPreferences)
```

First frame from the phone:

```json
{"t":"hello","v":1,"id":"<deviceId>","name":"Pixel 8 (Ayşe)","model":"Google Pixel 8","app":"1.1.0",
 "folders":[{"key":"k1","name":"Kamera","path":"/storage/emulated/0/DCIM/Camera"}],
 "caps":{"read":true,"receive":true}}
```

* `key` is a stable short id chosen by the phone per shared folder (not the path).
* `caps.read` — PC may list/read; `caps.receive` — PC may push files into shared folders.
  The phone ALWAYS refuses delete/rename/overwrite regardless of caps.

Server replies:

* Paired device with valid token → `{"t":"welcome","server":"<hostname>","paired":true}`.
* Unknown device / no token / a token the server no longer accepts (device was forgotten on
  the PC) → server creates a pending pairing, emits ServerEvent `{type:'pair', device}`
  (`device.code` = the code) to the clients that may approve it, and replies
  `{"t":"pending","code":"4821"}` (4-digit code shown on both screens so the user can
  match them). **When the phone receives `pending` it must discard any stored token.**
  When the PC user approves: `{"t":"paired","token":"<32 random bytes, base64url>"}`
  (server stores only sha256(token)); `paired` implies `welcome` — the server starts sending
  requests right after it. Rejected → `{"t":"rejected"}` and the socket closes (code 4403).
* Same deviceId connecting again (with a valid token) replaces the previous socket (old one
  closed with 4409). A wrong token never replaces an online device; it only creates a pending pairing.
* `hello` must be the first frame and arrive within 10 s; `X-Peek-Device` (if sent) must equal `hello.id`
  (`[A-Za-z0-9._-]{8,64}`); folder keys are `[A-Za-z0-9_-]{1,64}`; at most 64 folders.
* Close codes: 4400 bad/missing hello, 4401 device forgotten on the PC, 4403 rejected,
  4408 no hello in time, 4409 replaced by a newer connection, 4426 unsupported `v`,
  4429 too many pending pairings, 1001 server shutting down / sharing stopped.

Who approves (server rule):
* desktop app: only the local window (requests that did not come through the LAN port);
  LAN clients get 403 on approve/reject/forget and never receive `pair` events;
* server mode (Docker, no local window): any web client may approve; every events socket gets `pair`;
* a device can never approve itself (`X-Peek-Device` = the pending id → 403).

Heartbeat: server pings every 25 s (WS ping); phone sends `{"t":"ping"}` every 30 s and
expects `{"t":"pong"}`; 75 s without traffic → reconnect with backoff 1,2,4,8…60 s (+jitter).

## 2. Requests (server → phone)

```json
{"t":"req","rid":"<22-char base64url = 128 random bits>","op":"<op>","args":{…}}
{"t":"cancel","rid":"…"}
```

`rid` is single-use, bound to the device socket; the reply PUT must come from the same
device token. Unknown/expired rid → 410.

| op | args | reply |
|---|---|---|
| `list` | `{"key":"k1","dir":"Sub/Dir","recursive":true}` | PUT, `Content-Type: application/x-ndjson`, one JSON object per line: `{"p":"Sub/Dir/IMG_1.jpg","k":"image","s":123,"m":1700000000000,"w":4000,"h":3000,"d":null,"t":1699999999000,"id":12345}` (p = path relative to folder root, k = kind, s = size, m = mtime ms, w/h = oriented dims or null, d = duration s or null, t = taken instant ms or null, id = MediaStore _ID or null). Directories are implied by paths; empty directories as `{"dir":"Sub/Empty"}`. |
| `thumb` | `{"key":"k1","p":"Sub/IMG_1.jpg","s":320}` (s ∈ 320,640,1600; the server usually asks 640 and derives 320 itself) | PUT, `Content-Type: image/jpeg`, `Content-Length` |
| `file` | `{"key":"k1","p":"…","range":"bytes=0-1048575"}` (range optional) | PUT with `X-Peek-Status: 200|206|416`, `Content-Type`, `Content-Length`, `X-Peek-Content-Range` (206), `X-Peek-Total` (full size), `X-Peek-Mtime` (ms) |
| `display` | `{"key":"k1","p":"…","max":4096}` | PUT, JPEG ≤ max px |
| `strip` | `{"key":"k1","p":"…","n":10,"h":90}` | PUT, JPEG sprite |
| `details` | `{"key":"k1","p":"…"}` | PUT, `application/json`: `{"exif":{…},"gps":{"lat":…,"lon":…}|null,"make":…,"model":…,"lens":…}` |
| `receive` | `{"key":"k1","dir":"From PC","name":"IMG_9.jpg","size":123,"mtime":1700000000000,"url":"/api/devices/pull/<one-time token>"}` | phone GETs `url` (same base + Bearer + X-Peek-Device), writes `<name>.part`, renames to a unique name (`IMG_9 (1).jpg` on clash), sets mtime (`X-Peek-Mtime` of the GET, else `args.mtime`), MediaScanner; then text frame `{"t":"res","rid":"…","ok":true,"data":{"p":"From PC/IMG_9.jpg"}}` (`dir` may not exist yet: create it). |

Errors (any op): text frame `{"t":"err","rid":"…","status":404,"message":"not found"}`
instead of a PUT. Status codes: 400 bad args, 403 path outside shared folder / not allowed,
404 missing, 409 exists, 413 too large, 500 other, 503 busy.

Path rules (phone side, mandatory): `p`/`dir` are relative, `/`-separated, no `..`, no
leading `/`; resolve against the shared folder root, canonicalise (follow symlinks) and
require the result to stay inside the root's canonical path.

Reply PUT headers: `X-Peek-Status` (default 200; only `file` uses 206/416 — every other op
must answer 200 or send an `err` frame), `Content-Length` when known (required for `file`).
The server answers the PUT with 204 when the consumer finished, 499 if the consumer went away
(the response carries `Connection: close` and the server drops the connection right after:
phone must stop reading/sending and close its source), 410 for unknown / expired / already
answered rid, 401/403 for auth. An `err` frame for a rid whose PUT already started is ignored.
`cancel` may arrive before or during the PUT; after a `cancel` the phone must not start a PUT
for that rid (it would get 410) and should abort a running one.

Pull GET (`receive`): `200`, `Content-Type: application/octet-stream`, `Content-Length`,
`Last-Modified`, `X-Peek-Mtime` (ms). No Range support; one-time (a second GET → 404), valid
10 min, only for the device it was issued to (another device → 403). Timing: the GET must start
within 30 s of the `req` (else 504 + `cancel`); after the body was fully read the phone must
send `res`/`err` within 60 s.

Limits: the server keeps at most 8 requests in flight per device (priority: visible
thumbnails > file/display/details/strip/receive > background thumbs/list; background work never
uses more than 4 of the 8 slots); first byte timeout 20 s (thumb/list/display/strip/details)
or 30 s (file/receive) → consumer gets 504 and a `cancel` is sent. A request is "in flight"
from the `req` frame until its PUT body ended (or `err`/`res`). `list` is always sent with
`recursive:true`; on `changed` the server re-lists each reported dir sub tree (a `list` for a
dir that no longer exists should answer `err` 404 — the server then drops that sub tree).

## 3. Phone → server notifications

```json
{"t":"folders","folders":[…same shape as hello…]}
{"t":"changed","key":"k1","dirs":["Sub/Dir"]}
{"t":"pong"} / {"t":"ping"}
```

On `changed` the server re-lists that folder (debounced 2 s).

## 4. Server-side model

Who sees device folders (`/api/libraries` → `devices`, `/api/devices`, browse/media by id):
* server mode (Docker): every client;
* desktop: the local window, and paired phones that send `Authorization: Bearer <token>` +
  `X-Peek-Device` on their API calls (e.g. a phone in client mode); other LAN clients get
  `devices: []` and 404 for device folder/media ids;
* a request with `X-Peek-Device: <id>` never sees that device's own folders (404).
Device media are not part of the home timeline / Explore / search (like temp sessions);
they are browsed through their folder (`/api/browse?folderId=…`, Explore with `folderId` works).

Each shared folder becomes a regular `folders` row (`device_id`, `device_key`, path =
`device://<deviceId>/<key>`), listed under "Devices" (never inside a library). Its media rows are
regular `media` rows filled from `list` (meta_state=1); thumbnails come through `thumb`
and are cached by ThumbService keyed by (device, path, mtime, size). When the device is
offline, cached thumbnails still show; file requests answer 503 `device offline`.

PC-side write protection: rename/delete/move-from/move-to/mkdir/upload/edit-save/zip
peek+extract/folder rename+remove/video-edit+slideshow sources on device folders → 403.
Allowed: copy FROM a device folder into a PC folder (transfer job: `file` op, preserve
mtime) and copy INTO a device folder (transfer job: `receive` op; requires `caps.receive`).
Both are `POST /api/fs/move` with `copy:true`; the response is `{moved:[], job}`.
Renditions: `display` (viewer zoom, cached), `strip` (video editor, cached), `/preview` →
redirect to `/file`, `/stream` (transcode) → ffmpeg reads the server's own ranged `/file`
endpoint, i.e. a series of `file` requests with `range`. `details` refines the row
(GPS → place, camera, EXIF capture clock) whenever the viewer opens the info panel.

## 5. Golden transcripts

`packages/shared/fixtures/link/*.json` — arrays of `{dir:"p2s"|"s2p"|"put"|"get", …}`
steps used by both the Node fake-phone tests and the Kotlin JVM unit tests (codec).
