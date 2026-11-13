//! First start after the Electron app: carry the web UI's device preferences over.
//!
//! The React UI kept its per-device preferences in Chromium's `localStorage` (zustand stores saved
//! through `store/persist.ts`: `peek.prefs` = the `Prefs` object of `store/prefs.ts`,
//! `peek.slideshow` = the slideshow defaults of `features/slideshow/show.ts`). The page origin was
//! the embedded server, `http://127.0.0.1:<port>` (`desktop.json` → `port`, else the first free of
//! 47213–47216). Electron keeps that storage in `<userData>/Local Storage/leveldb`.
//!
//! ## Chromium's localStorage LevelDB format
//! (`components/services/storage/dom_storage/local_storage_impl.cc`, `dom_storage_database.cc`,
//! `storage_area_impl.cc`; `blink::StorageKey::SerializeForLocalStorage()` for the origin part)
//!
//! | key | value |
//! |---|---|
//! | `VERSION` | `"1"` (schema version) |
//! | `META:<origin>` / `METAACCESS:<origin>` | protobuf (last modified / accessed, size) — ignored |
//! | `_<origin>\x00<script key>` | `<script value>` |
//!
//! `<origin>` is the serialized origin without a trailing slash (`http://127.0.0.1:47213`). Script
//! keys and values are JavaScript strings with a one-byte format prefix
//! (`LocalStorageStringFormat` in `local_storage_impl.cc`): `0x00` = UTF-16LE code units,
//! `0x01` = Latin-1 (every code unit < 256, one byte each). The DB uses LevelDB's bytewise
//! comparator and snappy-compressed tables, which `rusty-leveldb` reads.
//!
//! The directory is copied to a temp dir first (without the `LOCK` file) so the Electron profile
//! is never written to, a running Electron's lock does not matter, and LevelDB's recovery writes
//! go to the copy. If LevelDB cannot open the copy (corruption, missing table), the write-ahead
//! logs are scanned directly as a fallback. Nothing here can fail the start: every problem ends
//! in "nothing migrated".

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pa_ui::prefs::Prefs;

/// `localStorage` contents: (origin, key) → value
pub type Storage = HashMap<(String, String), String>;

/// the leveldb directory of an Electron userData
pub fn leveldb_dir(user_data: &Path) -> PathBuf {
    user_data.join("Local Storage").join("leveldb")
}

/// A script string (`0x00` UTF-16LE / `0x01` Latin-1 prefix) → String.
pub fn decode_script_string(b: &[u8]) -> Option<String> {
    match b.split_first()? {
        (0, rest) if rest.len() % 2 == 0 => {
            let units: Vec<u16> = rest
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            Some(String::from_utf16_lossy(&units))
        }
        (1, rest) => Some(rest.iter().map(|&c| c as char).collect()),
        _ => None,
    }
}

/// The inverse of [`decode_script_string`] (Chromium picks Latin-1 when every unit fits).
#[cfg(test)]
pub fn encode_script_string(s: &str) -> Vec<u8> {
    if s.chars().all(|c| (c as u32) < 256) {
        let mut v = vec![1u8];
        v.extend(s.chars().map(|c| c as u8));
        v
    } else {
        let mut v = vec![0u8];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }
}

/// `_<origin>\x00<encoded key>` → (origin, key)
pub fn parse_key(k: &[u8]) -> Option<(String, String)> {
    let rest = k.strip_prefix(b"_")?;
    let nul = rest.iter().position(|&b| b == 0)?;
    let origin = std::str::from_utf8(&rest[..nul]).ok()?.to_string();
    let key = decode_script_string(&rest[nul + 1..])?;
    Some((origin, key))
}

/// Read every localStorage entry of a Chromium / Electron `Local Storage/leveldb` directory.
pub fn read_storage(dir: &Path) -> Result<Storage, String> {
    if !dir.join("CURRENT").is_file() && !has_logs(dir) {
        return Err("no leveldb".into());
    }
    let tmp = std::env::temp_dir().join(format!(
        "peekaround-ls-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let r = copy_db(dir, &tmp).and_then(|_| {
        // rusty-leveldb panics on some malformed inputs: never take the app down with it
        match std::panic::catch_unwind(|| read_with_leveldb(&tmp)) {
            Ok(r) => r,
            Err(_) => Err("leveldb reader panicked".into()),
        }
    });
    let out = match r {
        Ok(s) => Ok(s),
        Err(e) => {
            log::warn!("localStorage leveldb: {e}; scanning the logs instead");
            std::panic::catch_unwind(|| read_logs(dir))
                .unwrap_or_else(|_| Err("log scan panicked".into()))
                .map_err(|e2| format!("{e}; {e2}"))
        }
    };
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

fn has_logs(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".log"))
    })
}

fn copy_db(dir: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name();
        if name == "LOCK" || !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        // a file that cannot be read (locked by a running Electron) is skipped; LevelDB will
        // complain if it was needed and the log scan takes over
        let _ = std::fs::copy(e.path(), to.join(&name));
    }
    Ok(())
}

fn read_with_leveldb(dir: &Path) -> Result<Storage, String> {
    use rusty_leveldb::{LdbIterator, Options, DB};
    let opt = Options {
        create_if_missing: false,
        paranoid_checks: false,
        ..Options::default()
    };
    let mut db = DB::open(dir, opt).map_err(|e| e.to_string())?;
    let mut it = db.new_iter().map_err(|e| e.to_string())?;
    let mut out = Storage::new();
    while let Some((k, v)) = it.next() {
        if let (Some(ok), Some(val)) = (parse_key(&k), decode_script_string(&v)) {
            out.insert(ok, val);
        }
    }
    drop(it);
    let _ = db.close();
    Ok(out)
}

/// Fallback: replay the write-ahead logs (`*.log`, oldest first). LevelDB log format
/// (`doc/log_format.md`): 32 KiB blocks of records `crc32(4) len(2) type(1) data`; FULL = 1,
/// FIRST = 2, MIDDLE = 3, LAST = 4. A record is a WriteBatch (`write_batch.cc`): `seq(8) count(4)`
/// then entries `0x01 varint32 key varint32 value` (put) / `0x00 varint32 key` (delete).
pub fn read_logs(dir: &Path) -> Result<Storage, String> {
    let mut logs: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let num = n.strip_suffix(".log")?.parse::<u64>().ok()?;
            Some((num, e.path()))
        })
        .collect();
    if logs.is_empty() {
        return Err("no logs".into());
    }
    logs.sort();
    let mut out = Storage::new();
    for (_, p) in logs {
        let Ok(data) = std::fs::read(&p) else {
            continue;
        };
        for batch in log_records(&data) {
            apply_batch(&batch, &mut out);
        }
    }
    Ok(out)
}

fn log_records(data: &[u8]) -> Vec<Vec<u8>> {
    const BLOCK: usize = 32 * 1024;
    let mut out = vec![];
    let mut cur: Option<Vec<u8>> = None;
    let mut pos = 0;
    while pos + 7 <= data.len() {
        let left_in_block = BLOCK - pos % BLOCK;
        if left_in_block < 7 {
            pos += left_in_block; // trailer padding
            continue;
        }
        let len = u16::from_le_bytes([data[pos + 4], data[pos + 5]]) as usize;
        let ty = data[pos + 6];
        let start = pos + 7;
        let end = start + len;
        if end > data.len() || len > left_in_block - 7 {
            break; // torn tail
        }
        let frag = &data[start..end];
        match ty {
            1 => {
                out.push(frag.to_vec());
                cur = None;
            }
            2 => cur = Some(frag.to_vec()),
            3 => {
                if let Some(c) = cur.as_mut() {
                    c.extend_from_slice(frag);
                }
            }
            4 => {
                if let Some(mut c) = cur.take() {
                    c.extend_from_slice(frag);
                    out.push(c);
                }
            }
            0 if len == 0 => {
                // preallocated zero block (mmap'd logs): skip to the next block
                pos += left_in_block;
                continue;
            }
            _ => cur = None,
        }
        pos = end;
    }
    out
}

fn varint32(b: &[u8], pos: &mut usize) -> Option<usize> {
    let mut v: u32 = 0;
    for shift in (0..35).step_by(7) {
        let byte = *b.get(*pos)?;
        *pos += 1;
        v |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return Some(v as usize);
        }
    }
    None
}

fn slice<'a>(b: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    let n = varint32(b, pos)?;
    let s = b.get(*pos..*pos + n)?;
    *pos += n;
    Some(s)
}

fn apply_batch(b: &[u8], out: &mut Storage) {
    if b.len() < 12 {
        return;
    }
    let count = u32::from_le_bytes([b[8], b[9], b[10], b[11]]);
    let mut pos = 12;
    for _ in 0..count {
        let Some(&tag) = b.get(pos) else { return };
        pos += 1;
        let Some(k) = slice(b, &mut pos) else { return };
        match tag {
            1 => {
                let Some(v) = slice(b, &mut pos) else { return };
                if let (Some(ok), Some(val)) = (parse_key(k), decode_script_string(v)) {
                    out.insert(ok, val);
                }
            }
            0 => {
                if let Some(ok) = parse_key(k) {
                    out.remove(&ok);
                }
            }
            _ => return,
        }
    }
}

/// The origin the Electron app's page had: `desktop.json` port first, then the ports
/// main.ts `pickPort` tried, then any other loopback origin that has PeekAround prefs.
pub fn pick_origin(s: &Storage, port: Option<u16>) -> Option<String> {
    let has = |o: &str| s.contains_key(&(o.to_string(), "peek.prefs".to_string()));
    let mut want: Vec<u16> = port.into_iter().collect();
    want.extend([47213, 47214, 47215, 47216]);
    for p in want {
        for host in ["127.0.0.1", "localhost"] {
            let o = format!("http://{host}:{p}");
            if has(&o) {
                return Some(o);
            }
        }
    }
    let mut others: Vec<&String> = s
        .keys()
        .filter(|(o, k)| {
            k == "peek.prefs"
                && (o.starts_with("http://127.0.0.1:") || o.starts_with("http://localhost:"))
        })
        .map(|(o, _)| o)
        .collect();
    others.sort();
    others.first().map(|o| (*o).clone())
}

/// Map the web UI's stores onto the native prefs. Every key the native [`Prefs`] has is taken
/// (same camelCase names and values); unknown keys (`recursive`, `gridZoomMobile`,
/// `navCollapsed`, …) and values of the wrong type are dropped. Returns the prefs and how many
/// values were taken.
pub fn map_prefs(prefs_json: Option<&str>, slideshow_json: Option<&str>) -> (Prefs, usize) {
    let mut p = Prefs::default();
    let mut n = 0;
    if let Some(serde_json::Value::Object(o)) =
        prefs_json.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
    {
        for (k, v) in o {
            if k == "libraryView" {
                if let serde_json::Value::Object(m) = v {
                    for (lk, lv) in m {
                        if let Some(s) = lv.as_str().filter(|s| *s == "all" || *s == "folders") {
                            p.library_view.insert(lk, s.to_string());
                            n += 1;
                        }
                    }
                }
                continue;
            }
            let s = match &v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null
                | serde_json::Value::Array(_)
                | serde_json::Value::Object(_) => continue,
                other => other.to_string(),
            };
            if p.set_by_key(&k, &s) {
                n += 1;
            }
        }
    }
    if let Some(serde_json::Value::Object(o)) =
        slideshow_json.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
    {
        if let Some(d) = o.get("duration").and_then(|d| d.as_f64()) {
            if p.set_by_key("slideDuration", &d.to_string()) {
                n += 1;
            }
        }
        if let Some(t) = o.get("transition").and_then(|t| t.as_str()) {
            if p.set_by_key("slideTransition", t) {
                n += 1;
            }
        }
    }
    (p.sanitized(), n)
}

/// The web UI's prefs from an Electron userData, if there are any.
pub fn migrate(user_data: &Path, port: Option<u16>) -> Option<Prefs> {
    let dir = leveldb_dir(user_data);
    if !dir.is_dir() {
        return None;
    }
    let storage = match read_storage(&dir) {
        Ok(s) => s,
        Err(e) => {
            log::warn!(
                "Electron localStorage not readable ({}): {e}",
                dir.display()
            );
            return None;
        }
    };
    let origin = pick_origin(&storage, port)?;
    let get = |k: &str| {
        storage
            .get(&(origin.clone(), k.to_string()))
            .map(String::as_str)
    };
    let (p, n) = map_prefs(get("peek.prefs"), get("peek.slideshow"));
    log::info!("migrated {n} UI preferences from the Electron app ({origin})");
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pa-eprefs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ls_key(origin: &str, key: &str) -> Vec<u8> {
        let mut k = format!("_{origin}").into_bytes();
        k.push(0);
        k.extend(encode_script_string(key));
        k
    }

    #[test]
    fn script_strings() {
        assert_eq!(decode_script_string(b"\x01ab\xe7").as_deref(), Some("abç"));
        assert_eq!(
            decode_script_string(&[0, b'g', 0, 0x1f, 0x01]).as_deref(),
            Some("gğ")
        );
        assert_eq!(decode_script_string(b"\x02x"), None);
        assert_eq!(decode_script_string(&[0, 1]), None);
        for s in ["peek.prefs", "{\"a\":\"ç\"}", "Fotoğraflar 📷"] {
            assert_eq!(
                decode_script_string(&encode_script_string(s)).as_deref(),
                Some(s)
            );
        }
        assert_eq!(encode_script_string("ç")[0], 1);
        assert_eq!(encode_script_string("ğ")[0], 0);
        assert_eq!(
            parse_key(b"_http://127.0.0.1:47213\x00\x01peek.prefs"),
            Some(("http://127.0.0.1:47213".into(), "peek.prefs".into()))
        );
        assert_eq!(parse_key(b"META:http://127.0.0.1:47213"), None);
        assert_eq!(parse_key(b"VERSION"), None);
    }

    #[test]
    fn mapping_takes_known_keys_only() {
        let (p, n) = map_prefs(
            Some(
                r##"{"lang":"en","theme":"light","accent":"#10b981","motion":"reduced","gridMode":"square",
                "rowHeight":300,"sort":"name","order":"asc","kind":"video","recursive":true,"wideSidebar":true,
                "infoOpen":true,"hoverPreview":false,"gridZoomMobile":2,"gridZoomDesktop":3,"dateHeaders":false,
                "groupBy":"month","tileNames":true,"tileDuration":false,"tileFavorites":false,"confirmDelete":false,
                "viewerBg":"black","filmstrip":false,"infoDefault":"open","zoomStep":2,"dblZoom":4,
                "videoAutoplay":false,"videoMuted":true,"loopShortSec":10,"videoSpeed":1.5,"rememberVolume":false,
                "videoVolume":0.25,"showMemories":false,"weekStart":"sun","dateFormat":"iso","hourCycle":"h12",
                "hideLocation":true,"libraryView":{"lib:1":"folders","dir:2":"all","lib:9":"bogus"},
                "folderSort":"count","navCollapsed":{"albums":true},"theme2":"x"}"##,
            ),
            Some(r#"{"duration":7,"transition":"zoom","kenBurns":false,"res":720}"#),
        );
        let want = Prefs {
            lang: "en".into(),
            theme: "light".into(),
            accent: "#10b981".into(),
            motion: "reduced".into(),
            wide_sidebar: true,
            date_format: "iso".into(),
            hour_cycle: "h12".into(),
            week_start: "sun".into(),
            sort: "name".into(),
            order: "asc".into(),
            grid_mode: "square".into(),
            row_height: 300.0,
            grid_zoom_desktop: 3,
            info_open: true,
            info_default: "open".into(),
            viewer_bg: "black".into(),
            filmstrip: false,
            zoom_step: 2.0,
            dbl_zoom: 4.0,
            video_autoplay: false,
            video_muted: true,
            loop_short_sec: 10.0,
            video_speed: 1.5,
            remember_volume: false,
            video_volume: 0.25,
            hide_location: true,
            confirm_delete: false,
            kind: "video".into(),
            group_by: "month".into(),
            date_headers: false,
            tile_names: true,
            tile_duration: false,
            tile_favorites: false,
            hover_preview: false,
            show_memories: false,
            folder_sort: "count".into(),
            library_view: [
                ("lib:1".to_string(), "folders".to_string()),
                ("dir:2".to_string(), "all".to_string()),
            ]
            .into_iter()
            .collect(),
            slide_duration: 7.0,
            slide_transition: "zoom".into(),
            slideshow: Default::default(),
        };
        assert_eq!(p, want);
        // 36 scalar prefs + 2 library views + 2 slideshow values
        assert_eq!(n, 40);
        // garbage never fails
        let (p, n) = map_prefs(Some("not json"), Some("[1]"));
        assert_eq!((p.theme.as_str(), n), ("dark", 0));
        let (p, _) = map_prefs(
            Some(r#"{"theme":7,"rowHeight":"big","accent":"red"}"#),
            None,
        );
        assert_eq!(
            (p.theme.as_str(), p.row_height, p.accent.as_str()),
            ("dark", 220.0, "#8b5cf6")
        );
    }

    /// A LevelDB written with Chromium's key / value encoding (VERSION, META:, `_origin\0key`),
    /// Latin-1 and UTF-16 strings, two origins, an overwritten and a deleted value.
    #[test]
    fn synthetic_chromium_leveldb() {
        use rusty_leveldb::{Options, DB};
        let ud = tmp("synth");
        let dir = leveldb_dir(&ud);
        std::fs::create_dir_all(&dir).unwrap();
        {
            let mut db = DB::open(
                &dir,
                Options {
                    create_if_missing: true,
                    ..Options::default()
                },
            )
            .unwrap();
            db.put(b"VERSION", b"1").unwrap();
            let o = "http://127.0.0.1:47214";
            db.put(format!("META:{o}").as_bytes(), b"\x08\x01\x10\x02")
                .unwrap();
            db.put(
                &ls_key(o, "peek.prefs"),
                &encode_script_string(r##"{"theme":"system","lang":"en"}"##),
            )
            .unwrap();
            db.flush().unwrap(); // into a table, then overwrite in the log
            db.put(
                &ls_key(o, "peek.prefs"),
                &encode_script_string(
                    r##"{"theme":"light","accent":"#ef4444","libraryView":{"dir:Fotoğraflar":"all"}}"##,
                ),
            )
            .unwrap();
            db.put(
                &ls_key(o, "peek.slideshow"),
                &encode_script_string(r#"{"duration":9}"#),
            )
            .unwrap();
            db.put(&ls_key(o, "peek.splash"), &encode_script_string("1"))
                .unwrap();
            db.delete(&ls_key(o, "peek.splash")).unwrap();
            // another origin (a dev server): not picked while 47214 has prefs
            db.put(
                &ls_key("http://localhost:5173", "peek.prefs"),
                &encode_script_string(r#"{"theme":"dark"}"#),
            )
            .unwrap();
            db.close().unwrap();
        }
        let s = read_storage(&dir).unwrap();
        assert_eq!(
            s.get(&("http://127.0.0.1:47214".into(), "peek.slideshow".into()))
                .map(String::as_str),
            Some(r#"{"duration":9}"#)
        );
        assert!(!s.contains_key(&("http://127.0.0.1:47214".into(), "peek.splash".into())));
        // the log scan alone sees the same live values (the first put went into a table)
        let logs = read_logs(&dir).unwrap();
        assert_eq!(
            logs.get(&("http://127.0.0.1:47214".into(), "peek.prefs".into())),
            s.get(&("http://127.0.0.1:47214".into(), "peek.prefs".into()))
        );
        assert_eq!(
            pick_origin(&s, None).as_deref(),
            Some("http://127.0.0.1:47214")
        );
        assert_eq!(
            pick_origin(&s, Some(5173)).as_deref(),
            Some("http://localhost:5173")
        );
        let p = migrate(&ud, None).unwrap();
        assert_eq!(
            (
                p.theme.as_str(),
                p.accent.as_str(),
                p.slide_duration,
                p.lang.as_str()
            ),
            ("light", "#ef4444", 9.0, Prefs::default().lang.as_str())
        );
        assert_eq!(
            p.library_view.get("dir:Fotoğraflar").map(String::as_str),
            Some("all")
        );
        std::fs::remove_dir_all(ud).ok();
    }

    #[test]
    fn missing_or_corrupt_profiles_migrate_nothing() {
        let ud = tmp("corrupt");
        assert!(migrate(&ud, None).is_none());
        let dir = leveldb_dir(&ud);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("CURRENT"), "MANIFEST-000009\n").unwrap();
        std::fs::write(dir.join("000003.log"), b"\x00\x01garbage that is not a log").unwrap();
        std::fs::write(dir.join("LOCK"), b"").unwrap();
        assert!(migrate(&ud, None).is_none());
        std::fs::remove_dir_all(ud).ok();
    }

    /// A real profile: Chromium (Playwright persistent context) opened the React UI from the Rust
    /// server on http://127.0.0.1:47213 and changed the prefs through the app's own stores — see
    /// tests/fixtures/electron-localstorage/README.md.
    #[test]
    fn real_chromium_profile() {
        let fx = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/electron-localstorage");
        let ud = tmp("real");
        let dst = leveldb_dir(&ud);
        std::fs::create_dir_all(&dst).unwrap();
        for e in std::fs::read_dir(fx.join("leveldb")).unwrap().flatten() {
            std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
        }
        let before: Vec<_> = std::fs::read_dir(&dst)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        let want: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fx.join("expected.json")).unwrap())
                .unwrap();
        let p = migrate(&ud, Some(47213)).expect("prefs migrated");
        let got = serde_json::to_value(&p).unwrap();
        for (k, v) in want.as_object().unwrap() {
            assert_eq!(&got[k], v, "{k}");
        }
        let after: Vec<_> = std::fs::read_dir(&dst)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(before, after, "the Electron profile must not be modified");
        std::fs::remove_dir_all(ud).ok();
    }
}
