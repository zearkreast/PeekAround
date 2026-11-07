A real Chromium localStorage LevelDB (`<profile>/Default/Local Storage/leveldb`, the same layout as
Electron's `<userData>/Local Storage/leveldb`), made with Playwright's Chromium (build 1243):

1. the Rust server served the React UI: `peekaround --root Test=<fixtures/library copy> --port 47213
   --host 127.0.0.1 --web packages/web/dist`
2. a persistent context opened http://127.0.0.1:47213/, stored `peek.prefs` / `peek.slideshow` in the
   app's persist format (`JSON.stringify` of the whole store), reloaded, then clicked Settings →
   Appearance → Light so the app itself rewrote `peek.prefs` (theme "light")
3. the context was reopened twice (Chromium reuses the log: the data stays in `000003.log`)

`peek.prefs` contains `navCollapsed: {"Fotoğraflar": true}`, so its value is stored UTF-16LE (format
byte 0x00); keys and `peek.slideshow` are Latin-1 (0x01). `expected.json` = the native prefs after
the migration (src/electron_prefs.rs, test `real_chromium_profile`).
