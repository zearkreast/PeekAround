# pa-ui — the PeekAround Slint UI

All screens + their view models, independent of where the data comes from. The binaries
(`pa-desktop`, later `pa-android` and the wasm build) create a [`backend::Backend`] and a tokio
runtime and call [`run`].

```
ui/                     .slint sources (compiled by build.rs)
  theme.slint           design tokens (styles.css port), Motion (animation level), I18n global, fonts
  icons.slint           generated: lucide SVGs as images (tools/gen-icons.mjs + gen-icons-slint.sh)
  globals.slint         Shell (router, connection, toasts), GridState, PrefsState
  components/           Button, IconButton (+tooltip), TextField, Switch, Checkbox, Slider, Segmented,
                        ColorSwatches, Select, PopoverPanel/MenuItem/MenuLabel/MenuSep, Dialog, Sheet,
                        Toaster, Spinner, ProgressRing, ProgressBar, Badge, Kbd, Card, Skeleton, EmptyState, Icon
  shell/                nav rail, top bar (search, view options popover)
  pages/                grid (virtualized MediaGrid), photos (timeline / favorites / search), settings, placeholder
  app.slint             AppWindow (rail · top bar · page with transitions · toasts)
  gallery.slint         component gallery window (`peekaround-desktop --gallery [light]`)
src/
  backend.rs            Backend trait; HttpBackend (pa-client, feature `http`); DemoBackend (synthetic N items)
  rt.rs                 executor seam: futures on tokio, results back on the UI thread (slint::spawn_local)
  app.rs                shell wiring: router + back stack, prefs → globals, toasts, live events
  grid/layout.rs        exact justified/square layout (port of grid/layout.ts, same tests)
  grid/mod.rs           skeleton + paged items, slot-pool virtualization, thumbnail scheduling
  thumbs.rs             decode (WebP/JPEG/PNG) off-thread, byte-bounded LRU, thumbhash placeholders
  i18n.rs               TR/EN tables + Intl-compatible date formatting
  prefs.rs, theme.rs    ui-prefs.json; accent partner colours (useTheme.ts mix())
  perf.rs               frame timing, auto-scroll perf run, automation script (screenshots)
i18n/                   tr.json / en.json generated from packages/web/src/i18n (tools/gen-i18n.mjs) + native.json
fonts/                  Inter (variable) + Bricolage Grotesque 600/700 (OFL)
```

## Decisions

**Renderer: femtovg (OpenGL / GLES via glutin) on the winit backend.**
- Builds from crates.io alone: pure Rust, no C++ toolchain, no downloads at build time. Skia
  (`rust-skia`) fetches prebuilt binaries from GitHub during the build (blocked here, and a problem for
  offline / CI builds) or needs a multi-GB source build with python + ninja + clang.
- Runs under `xvfb-run` with Mesa (llvmpipe): every screenshot and perf number below comes from that.
- Same renderer on Android (GLES 2/3 through android-activity: feature `android`) and Windows (WGL/EGL
  through glutin), and femtovg can switch to its wgpu backend later if needed.
- Fallback to consider for Windows machines without GL 2.1 (VMs / RDP): add `renderer-software` and
  pick it with `SLINT_BACKEND=winit-software`. Not enabled yet (binary size).

**Window decorations: native** on Windows and Linux. No overlay title bar, so there are no drag-region /
click-through bugs like the Electron `titleBarOverlay` ones; the top bar is a plain app bar.

**i18n: Rust-side string table bound through a pure callback**, `I18n.t("key", I18n.rev)`. The keys,
`{var}` placeholders and "one|many" plurals are the web UI's (all 1271 keys are ported, so later screens
need no new translation work); switching the language bumps `I18n.rev`, which re-evaluates every
binding at runtime. Rust code (date headers, counts, toasts) uses the same table. Slint's `@tr` +
bundled gettext was the alternative, but it needs English source strings in the .slint files and a
second mechanism for Rust strings. A test checks that every key used in `ui/**/*.slint` exists.

**Virtualization:** a pool of tile slots in a `VecModel`. An item keeps its slot while it stays inside
the view (± ¾ screen of overscan), so a scrolled frame only rewrites the slots of entering rows (the
repeater instances are reused, not recreated). Headers are a separate small model.

## Running / checking

```
peekaround-desktop --server http://127.0.0.1:19250        # any PeekAround server (TS or Rust)
peekaround-desktop --demo 10000                            # synthetic library, no server
PA_SCRIPT="wait 3000; shot a.png; nav settings; set theme light; click 1404 30; type berlin\n; quit"
PA_AUTOSCROLL=40,12 PA_AUTOSCROLL_QUIT=1                   # px per 16 ms tick, seconds → [perf] report
```
Script steps: `wait ms`, `shot file.png`, `nav photos|favorites|explore|albums|library|settings`,
`set <pref> <value>`, `search <text>`, `scroll <px>`, `section <settings section>`, `toast <text>`,
`click x y`, `move x y`, `type <text>` (`\n` = Enter), `quit`.

Under xvfb, winit needs `libxkbcommon-x11-0` (and `libegl1` / `libegl-mesa0`).

## Licences

Lucide icons: ISC (`ui/icons/LICENSE-lucide.txt`). Fonts: SIL OFL 1.1 (`fonts/`).
