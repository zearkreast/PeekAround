# pa-meta ↔ TypeScript parity

`pa-meta` ports `packages/server/src/media/metadata.ts` (exifr 7.1.3 + sharp 0.35 / libvips 8.18 +
Google Takeout sidecars + `groupColumns`). The goal is identical DB columns for the same file, so the
Rust server can open an existing `peekaround.db` without re-reading every file.

## How parity is checked

| Test | What | Cases |
|---|---|---|
| `tests/parity.rs` | `extract_meta` (both with and without a known size), `read_exif`, `group_columns` in 4 home zones (UTC, Istanbul, New York, Lord Howe ±30 min DST) | 168 files: the fixture library (`scripts/make-fixtures.mjs`), 5 Nikon JPEGs with GPS + an iPhone HEIC (`tests/files/samples`), 70+ crafted EXIF / XMP / PNG / WebP / TIFF / AVIF / SVG / BMP / ICO / Takeout files (`tests/files/craft`) |
| `tests/parity.rs` | `video_group_meta`, `parse_video_location` | real ffprobe results of the fixture videos + 18 synthetic tag sets |
| `tests/fuzz_parity.rs` | the merged exifr output **tag by tag** (every key, not only the ones used) for both exifr calls | 700 generated files: random IFDs with every TIFF type, broken offsets / counts / types, string GPS values, big + little endian, random (also malformed) XMP packets, JPEG / PNG / TIFF containers |

All of them pass: the Rust output equals the TypeScript output (floats within 1e-9 relative).

Regenerate (needs the TypeScript checkout with `node_modules`, e.g. `/home/user/peekaround`, and ffmpeg):

```sh
python3 -I tests/tools/craft.py tests/files/craft          # crafted files (+ ffmpeg for the small videos, see the script)
cd <ts checkout> && TZ=UTC node_modules/.bin/tsx <this crate>/tests/tools/gen-meta.mts \
  <this crate>/tests/golden-meta.json library=<ts checkout>/fixtures/library \
  samples=<this crate>/tests/files/samples craft=<this crate>/tests/files/craft
TZ=UTC node tests/tools/gen-fuzz.mjs tests/golden-fuzz.json
node tests/tools/gen-tags.mjs > src/tags.rs                # exifr's tag dictionaries
```

## Design decisions

* **No EXIF crate.** `kamadak-exif` reads EXIF from JPEG/TIFF/HEIF/PNG/WebP but has no XMP, its own value
  typing and stricter error handling; `nom-exif` is larger (async, many formats) and also differs from
  exifr in typing and error granularity. Both would change extracted values on real libraries. Instead
  `src/tiff.rs` / `src/exifr.rs` / `src/xmp.rs` reproduce exifr's behaviour exactly (≈1k lines, no
  `unsafe`, bounds-checked, no panics on hostile input):
  * value typing: ASCII → trimmed string (`''` → undefined, which still overrides on merge), BYTE /
    UNDEFINED → `Uint8Array`, other lists → typed arrays (not `Array.isArray`), RATIONAL lists → plain
    arrays, `0/0` → NaN;
  * error granularity: a bad IFD0 drops the whole TIFF segment, a bad EXIF / GPS / Interop IFD keeps the
    tags read before the error; a throwing reviver (`readExif`) drops the segment;
  * `latitude` / `longitude` are exifr's `ConvertDMSToDD(...value, ref)` including its quirks (a string
    value is spread into characters and concatenated, the reference only lands in `direction` for
    3-part values);
  * merge order: XMP first, TIFF last (EXIF wins on conflicts), namespaces flattened by local name;
  * containers: JPEG (first Exif + first XMP APP1 before SOS), TIFF (IFD0 + its XMP tag 700), HEIC / AVIF
    (`Exif` item via `iinf` / `iloc`, only when the compatible brands list `heic` / `avif`), PNG (`eXIf`,
    `iTXt` XMP; `tEXt` only for `readExif`). **WebP, GIF, BMP, ICO and SVG have no EXIF** (exifr cannot read
    them) — kept, so `meta_v` stays meaningful.
* **XMP is parsed like exifr, not with quick-xml.** exifr finds `rdf:Description` and its attributes /
  children with regular expressions (no entity decoding, `xml:lang` turns an `rdf:li` into an object, a
  missing close tag skips the element, `"8000"` → number, `"True"` → boolean). A real XML parser would
  extract different values from the same packets (and nothing from slightly malformed ones). `src/xmp.rs`
  is a backtracking-free re-implementation of those expressions (the patterns use back-references, which
  the `regex` crate does not support), linear in the packet size.
* **Image size without decoding.** `src/dims.rs` reads headers the way the TS server's
  `sharp().metadata()` reported them: JPEG SOF (+ EXIF orientation 5–8 swaps), PNG IHDR (+ `eXIf`
  orientation), GIF logical screen + frame count, WebP VP8/VP8L/VP8X (+ EXIF orientation, ANMF frame
  count for animated files only), TIFF first IFD (+ page count), AVIF `ispe`/`irot` (+ top-level image
  count), SVG width/height (units at 72 DPI) or viewBox. Like sharp: BMP and ICO give no size, `.heic` /
  `.heif` are not probed, APNG reports no pages, the format is detected by content (a JPEG named `.png`
  works).
* `read_exif(path, local_tz)`: exifr revives `DateTimeOriginal` with `new Date(y, m, d, h, mi, s)`, i.e.
  in the **process** time zone of the TS server. The Rust server must pass its local zone (e.g. from the
  `iana-time-zone` crate; Docker: `UTC`).
* `ProbeResult` lives in `pa-core` (`pa_core::ProbeResult`); `tags` is an `IndexMap` because
  `videoGroupMeta` scans unknown vendor tags in ffprobe's order (video stream tags, then container tags
  overriding — `probeFromJson`). pa-media should fill it with `serde_json`'s `preserve_order` or an
  ordered walk.

## Known, deliberate differences

| Case | TypeScript | Rust | Why |
|---|---|---|---|
| JPEG metadata segments after several hundred KiB of other APP segments; metadata far into big PNG / TIFF files | exifr's chunked file reader may stop at its chunk limit (`chunkLimit`) | read from the real file (block-cached ranges, only the needed bytes) | an artefact of exifr's reader that loses real metadata; not reachable with the fixture / camera files |
| Truncated containers (PNG/HEIF box header cut at EOF) | exifr throws → no metadata at all | the boxes / chunks read so far are used | more robust; only on broken files |
| SVG with `em`/`ex`/`%` sizes, nested `<svg>` | librsvg rules | simplified (12 / 6 px per em / ex, % of the viewBox) | rare in photo libraries; dimensions are re-read by the thumbnailer |
| AVIF / HEIF `pages` | libheif's top-level image count | top-level images = coded items not referenced as `thmb` / `auxl` / `base` / grid tiles | same result for camera and sharp-written files |
