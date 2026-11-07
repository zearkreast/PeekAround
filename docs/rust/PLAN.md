# PeekAround → Rust rewrite plan

## Decisions (agreed with the user)
| Topic | Decision |
|---|---|
| Scope | **Everything in Rust**, UI included. No WebView, no Electron, no React, no Capacitor. |
| UI toolkit | **Slint** (GPLv3, matches the repo license). Native GPU rendering on Windows, Linux, Android. Renderer: `femtovg` or `skia` (decide in M2 by what builds reliably offline/cross). |
| Browser access | **Only the Docker server** is reachable from a browser: the same Slint UI compiled to **WebAssembly**, served by the Docker server. The desktop app's LAN share and the phone no longer use a browser. |
| Phone ↔ PC | The native Android app talks to the desktop app over the HTTP/WS API (a native client, no browser). The desktop LAN share stays (port 1313), serving the API to the Android app. Docker keeps 8080. |
| Android | Native Rust app (Slint Android backend + `android-activity`). Android APIs (MediaStore, permissions, share intents) through JNI from Rust. **A foreground service with a notification and a Stop action keeps phone folder sharing running with the screen off or the app in the background**, as in 1.3.2; the minimal Kotlin/Java the service needs is allowed. |
| Commit | Rust code is committed and pushed to a **separate branch** (`rust-rewrite`); `main` stays at 1.3.2 until 2.0 is merged. Comment cleanup before every commit, no Claude attribution. |
| Delivery | Besides the commits, built packages (Windows zip/installer, APK) go to `.local-project-files` for testing. No source zip any more. |
| Release | Each alpha is published as a GitHub **pre-release** tagged `v2.0.0-alpha.N`; 1.3.2 stays "Latest". |
| M4 order | Fonts → slideshow (player + builder) → photo editor → collage → video editor; one commit per part. |
| Photo editor | Adjustments and filters on the **GPU through wgpu** (like the 1.x WebGL renderer). |
| HEIC | **libheif** (`libheif-rs`), bundled in every package (Windows, Linux, Android, Docker). |
| Data compatibility | The Rust server opens the existing `peekaround.db` (same schema, migrations 001–005, `PRAGMA user_version`) so libraries, favorites, albums, settings, MEGA folders and paired phones survive the upgrade. Thumbnail caches may be regenerated. |
| API compatibility | The HTTP/WS API stays the same during the rewrite (routes, JSON shapes in `packages/shared/src/*.ts`) so the old React UI can be used as a test client for the Rust server (milestone M1), and old 1.3 phone apps keep working against a new PC for a transition period. |

## Feature changes the user accepted
- No browser access to the desktop app's LAN share (Docker only).
- No dragging files out of the app to the OS; "Show in folder", "Copy path" and "Copy/move to folder" replace it.
- No PWA for the desktop LAN share (Docker web UI can still be installed as a PWA if feasible with wasm).
Anything else that would change behaviour must be asked first.

## Workspace layout (Cargo workspace at the repo root)
```
Cargo.toml                (workspace)
crates/
  pa-core/                pure logic + API types (port of packages/shared: time, geo, classify, memories,
                          trips, search, takeout, sun, color, insights, tiles, albums, remoteFolders, index.ts types)
  pa-server/              axum HTTP/WS server (port of packages/server): db + migrations, security, scanner,
                          watcher, metadata, thumbs, browse/skeleton, explore/insights/duplicates, albums,
                          library, settings, cache, tiles proxy, events, jobs (ffmpeg: transcode, video edit,
                          slideshow export), zip, devices (phone link), remote (MEGA streaming + import), mdns
  pa-jobs/                job engine: queue + lanes, video transcode/stream sessions, video edit + slideshow export
                          (ffmpeg), MEGA import / phone transfer jobs — storage behind traits
  pa-link/                phone link: WS control plane, PUT reply data plane, pairing, device folders — traits
  pa-remote/              MEGA streaming remote folders service (listing, refresh, metadata, cache, quota) — traits
  pa-mega/                MEGA public-link client (API, AES-CTR/CBC/ECB crypto, attributes, ranged download, login)
  pa-media/               image decode/resize/encode (image, fast_image_resize, libheif-rs, webp), thumbhash,
                          ffmpeg CLI wrapper + `ffmpeg -i` parser
  pa-meta/                metadata extraction: EXIF/XMP (photo), Google Takeout sidecars, video tags → the
                          grouping columns (time zone resolution, GPS, camera, flags)
  pa-client/              typed API client (native: reqwest + tokio-tungstenite; wasm: fetch/WebSocket)
  pa-ui/                  Slint UI (all screens) + view models, backend-agnostic (talks to a `Backend` trait)
  pa-desktop/             desktop binary: embedded server + UI, tray, single instance, autostart, notifications,
                          update check, LAN share
  pa-android/             Android cdylib: UI + local mode (MediaStore via JNI) + client mode (pa-client)
  pa-web/                 wasm build of pa-ui for the Docker browser UI
  pa-cli/                 `peekaround` server binary for Docker / headless use
```
The TypeScript packages stay in `packages/` as the reference implementation until each milestone replaces them;
they are removed at the end (M6).

## Milestones (each ends with a working, tested build delivered to the user)
- **M1 — Rust server, API-compatible.** pa-core, pa-media, pa-mega, pa-server, pa-cli. Verified by an API
  conformance harness that runs the TS server and the Rust server on the same fixture library and diffs JSON
  responses route by route, by porting the server integration tests to Rust, and by running the existing React
  UI against the Rust server (Playwright). Docker image switches to the Rust server.
- **M2 — Slint desktop app, core.** Shell (window, tray, settings), gallery grid (justified, exact-layout skeleton,
  fast scroller, selection, drag & drop), viewer (zoom/pan, video playback via ffmpeg decode + audio), library hub,
  albums, favorites, search, settings, MEGA dialogs, devices/pairing, jobs panel, TR/EN i18n, themes, transitions.
- **M3 — Explore + map.** Explore sections, memory player, custom slippy-map widget with clustering (tile proxy).
- **M4 — Editors.** Photo editor (GPU adjustments via wgpu/femtovg shaders or CPU), collage, video editor,
  slideshow builder/player, fonts.
- **M5 — Android native app.** Local mode (MediaStore, permissions, share intents, downloads) + client mode
  (connect to PC/Docker, browse, upload/download, phone folder sharing through the foreground service), mDNS discovery, QR.
- **M6 — Docker web UI (wasm), packaging, CI.** Windows installer (MSI/NSIS via cargo-packager or WiX), Linux
  AppImage/deb/pacman, APK (signed with the user's keystore secrets), Docker multi-arch, GitHub Actions; remove the
  TypeScript packages; release notes.
