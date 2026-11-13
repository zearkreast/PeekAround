//! Pure pieces of the phone side (1.x `LinkCore.kt`): frame codec, NDJSON list lines, HTTP Range
//! parsing, `file` reply headers, path guard, unique names, reconnect backoff, kinds / MIME types.

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::prefs::LinkFolder;

/// protocol version (`?v=1`, `hello.v`)
pub const VERSION: u32 = 1;

/// An error answered as `{"t":"err","rid","status","message"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkError {
    /// HTTP-like status
    pub status: u16,
    /// message
    pub message: String,
}

impl LinkError {
    /// new error
    pub fn new(status: u16, message: impl Into<String>) -> LinkError {
        LinkError {
            status,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.message)
    }
}

impl From<std::io::Error> for LinkError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => LinkError::new(404, "not found"),
            std::io::ErrorKind::PermissionDenied => LinkError::new(403, "not readable"),
            _ => LinkError::new(500, e.to_string()),
        }
    }
}

/// A text frame from the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// pairing code to show; the stored token is no longer accepted
    Pending(String),
    /// approved: the new token
    Paired(String),
    /// paired device connected (server host name)
    Welcome(String),
    /// the PC refused
    Rejected,
    /// application ping
    Ping,
    /// application pong
    Pong,
    /// a request
    Req {
        /// request id
        rid: String,
        /// operation
        op: String,
        /// arguments
        args: Map<String, Value>,
    },
    /// cancel a request
    Cancel(String),
    /// anything else / malformed
    Unknown,
}

fn valid_rid(s: &str) -> bool {
    (8..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// folder keys: `[A-Za-z0-9_-]{1,64}`
pub fn valid_key(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn str_of<'a>(o: &'a Map<String, Value>, k: &str) -> &'a str {
    o.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Parse one text frame from the server.
pub fn parse(text: &str) -> Incoming {
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(text) else {
        return Incoming::Unknown;
    };
    match str_of(&o, "t") {
        "pending" => Incoming::Pending(str_of(&o, "code").to_string()),
        "paired" => match str_of(&o, "token") {
            "" => Incoming::Unknown,
            t => Incoming::Paired(t.to_string()),
        },
        "welcome" => Incoming::Welcome(str_of(&o, "server").to_string()),
        "rejected" => Incoming::Rejected,
        "ping" => Incoming::Ping,
        "pong" => Incoming::Pong,
        "req" => {
            let rid = str_of(&o, "rid");
            let op = str_of(&o, "op");
            if !valid_rid(rid) || op.is_empty() {
                return Incoming::Unknown;
            }
            let args = match o.get("args") {
                Some(Value::Object(a)) => a.clone(),
                _ => Map::new(),
            };
            Incoming::Req {
                rid: rid.to_string(),
                op: op.to_string(),
                args,
            }
        }
        "cancel" => {
            let rid = str_of(&o, "rid");
            if valid_rid(rid) {
                Incoming::Cancel(rid.to_string())
            } else {
                Incoming::Unknown
            }
        }
        _ => Incoming::Unknown,
    }
}

fn folders_json(folders: &[LinkFolder]) -> Value {
    Value::Array(
        folders
            .iter()
            .map(|f| json!({"key": f.key, "name": f.name, "path": f.path}))
            .collect(),
    )
}

/// first frame
pub fn hello(id: &str, name: &str, model: &str, app: &str, folders: &[LinkFolder]) -> String {
    json!({
        "t": "hello", "v": VERSION, "id": id, "name": name, "model": model, "app": app,
        "folders": folders_json(folders),
        "caps": {"read": true, "receive": true},
    })
    .to_string()
}

/// shared folders changed
pub fn folders(folders: &[LinkFolder]) -> String {
    json!({"t": "folders", "folders": folders_json(folders)}).to_string()
}

/// error reply
pub fn err(rid: &str, status: u16, message: &str) -> String {
    json!({"t": "err", "rid": rid, "status": status, "message": message}).to_string()
}

/// `receive` result
pub fn res(rid: &str, data: Value) -> String {
    json!({"t": "res", "rid": rid, "ok": true, "data": data}).to_string()
}

/// files changed under `dirs` of folder `key`
pub fn changed(key: &str, dirs: &[String]) -> String {
    json!({"t": "changed", "key": key, "dirs": dirs}).to_string()
}

/// `{"t":"ping"}`
pub fn ping() -> String {
    r#"{"t":"ping"}"#.into()
}

/// `{"t":"pong"}`
pub fn pong() -> String {
    r#"{"t":"pong"}"#.into()
}

/// JS-like number: 12.0 → "12", 12.5 → "12.5"
fn num(d: f64) -> String {
    if !d.is_finite() {
        "null".into()
    } else if d == d.floor() && d.abs() < 1e15 {
        (d as i64).to_string()
    } else {
        d.to_string()
    }
}

fn opt<T: ToString>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "null".into())
}

/// metadata of one list line
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LineMeta {
    /// oriented width
    pub w: Option<u32>,
    /// oriented height
    pub h: Option<u32>,
    /// seconds
    pub d: Option<f64>,
    /// taken instant (ms)
    pub t: Option<i64>,
    /// MediaStore _ID
    pub id: Option<i64>,
}

/// One NDJSON line of a `list` reply (field order of the golden transcripts).
pub fn list_line(p: &str, k: &str, s: u64, m: i64, meta: &LineMeta) -> String {
    format!(
        "{{\"p\":{},\"k\":{},\"s\":{},\"m\":{},\"w\":{},\"h\":{},\"d\":{},\"t\":{},\"id\":{}}}",
        Value::String(p.into()),
        Value::String(k.into()),
        s,
        m,
        opt(meta.w),
        opt(meta.h),
        meta.d.map(num).unwrap_or_else(|| "null".into()),
        opt(meta.t),
        opt(meta.id),
    )
}

/// an empty directory
pub fn dir_line(dir: &str) -> String {
    format!("{{\"dir\":{}}}", Value::String(dir.into()))
}

/// a parsed `Range` header
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// no / ignored range
    Full,
    /// inclusive byte range
    Part(u64, u64),
    /// 416
    Unsatisfiable,
}

/// Parse one `bytes=` range against `total` bytes (RFC 9110): `0-99`, `100-`, `-500`; several
/// ranges → the first; malformed / not bytes → Full.
pub fn parse_range(header: Option<&str>, total: u64) -> ByteRange {
    let Some(h) = header.map(str::trim) else {
        return ByteRange::Full;
    };
    if h.len() < 6 || !h[..6].eq_ignore_ascii_case("bytes=") {
        return ByteRange::Full;
    }
    let spec = h[6..].split(',').next().unwrap_or("").trim();
    let Some((a, b)) = spec.split_once('-') else {
        return ByteRange::Full;
    };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        let Ok(n) = b.parse::<i64>() else {
            return ByteRange::Full;
        };
        if n <= 0 || total == 0 {
            return ByteRange::Unsatisfiable;
        }
        return ByteRange::Part(total.saturating_sub(n as u64), total - 1);
    }
    let Ok(start) = a.parse::<i64>() else {
        return ByteRange::Full;
    };
    if start < 0 {
        return ByteRange::Full;
    }
    let start = start as u64;
    let end = if b.is_empty() {
        total.saturating_sub(1)
    } else {
        match b.parse::<u64>() {
            Ok(e) if e >= start => e,
            _ => return ByteRange::Full,
        }
    };
    if start >= total {
        return ByteRange::Unsatisfiable;
    }
    ByteRange::Part(start, end.min(total - 1))
}

/// status, byte window and headers of a `file` reply PUT
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReply {
    /// 200 / 206 / 416
    pub status: u16,
    /// first byte
    pub offset: u64,
    /// bytes to send
    pub length: u64,
    /// headers (Content-Length included)
    pub headers: Vec<(String, String)>,
}

/// headers of a `file` reply
pub fn file_reply(range: Option<&str>, total: u64, mtime: i64, mime: &str) -> FileReply {
    let h = |v: &[(&str, String)]| -> Vec<(String, String)> {
        v.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    };
    match parse_range(range, total) {
        ByteRange::Unsatisfiable => FileReply {
            status: 416,
            offset: 0,
            length: 0,
            headers: h(&[
                ("X-Peek-Status", "416".into()),
                ("X-Peek-Content-Range", format!("bytes */{total}")),
                ("X-Peek-Total", total.to_string()),
                ("Content-Length", "0".into()),
            ]),
        },
        ByteRange::Part(s, e) => FileReply {
            status: 206,
            offset: s,
            length: e - s + 1,
            headers: h(&[
                ("X-Peek-Status", "206".into()),
                ("Content-Type", mime.into()),
                ("Content-Length", (e - s + 1).to_string()),
                ("X-Peek-Content-Range", format!("bytes {s}-{e}/{total}")),
                ("X-Peek-Total", total.to_string()),
                ("X-Peek-Mtime", mtime.to_string()),
            ]),
        },
        ByteRange::Full => FileReply {
            status: 200,
            offset: 0,
            length: total,
            headers: h(&[
                ("X-Peek-Status", "200".into()),
                ("Content-Type", mime.into()),
                ("Content-Length", total.to_string()),
                ("X-Peek-Total", total.to_string()),
                ("X-Peek-Mtime", mtime.to_string()),
            ]),
        },
    }
}

fn outside() -> LinkError {
    LinkError::new(403, "outside shared folder")
}

/// Validate a protocol-relative path: `/`-separated, no `..` / `.` / empty segments, NUL, `\`,
/// drive letters or leading `/`. "" = the root.
pub fn clean_rel(rel: &str) -> Result<String, LinkError> {
    if rel.is_empty() {
        return Ok(String::new());
    }
    let b = rel.as_bytes();
    if rel.starts_with('/')
        || rel.contains('\0')
        || rel.contains('\\')
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
    {
        return Err(outside());
    }
    let segs: Vec<&str> = rel.trim_end_matches('/').split('/').collect();
    if segs.iter().any(|s| s.is_empty() || *s == "." || *s == "..") {
        return Err(outside());
    }
    Ok(segs.join("/"))
}

/// `p` is `root` or below it (both canonical)
pub fn is_inside(root: &Path, p: &Path) -> bool {
    p.starts_with(root)
}

/// Resolve `rel` inside `root` following symlinks: the canonical result must stay inside the
/// root's canonical path (403 otherwise; 404 when it does not exist).
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, LinkError> {
    let clean = clean_rel(rel)?;
    let base = root.canonicalize()?;
    if clean.is_empty() {
        return Ok(base);
    }
    let mut p = base.clone();
    for s in clean.split('/') {
        p.push(s);
    }
    let c = p.canonicalize()?;
    if !is_inside(&base, &c) {
        return Err(outside());
    }
    Ok(c)
}

/// `/`-separated path of `p` relative to `root` ("" for the root itself)
pub fn rel_of(root: &Path, p: &Path) -> String {
    match p.strip_prefix(root) {
        Ok(r) => r
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => String::new(),
    }
}

/// A plain file name from the PC: no separators, not `.`/`..`, no control chars, ≤ 255.
pub fn clean_name(name: &str) -> Result<String, LinkError> {
    let n = name.trim();
    if n.is_empty()
        || n == "."
        || n == ".."
        || n.contains('/')
        || n.contains('\\')
        || n.chars().any(|c| c < ' ')
        || n.chars().count() > 255
    {
        return Err(LinkError::new(400, "bad file name"));
    }
    Ok(n.to_string())
}

/// "IMG_1.jpg" → "IMG_1 (1).jpg", "IMG_1 (2).jpg"…; dot-files / names without an extension keep
/// the whole name as base.
pub fn unique_name(name: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(name) {
        return name.to_string();
    }
    let (base, ext) = match name.rfind('.') {
        Some(d) if d > 0 => (&name[..d], &name[d..]),
        _ => (name, ""),
    };
    let mut i = 1;
    loop {
        let c = format!("{base} ({i}){ext}");
        if !exists(&c) {
            return c;
        }
        i += 1;
    }
}

/// attempt 0 → ~1 s, 1 → ~2 s, … capped at 60 s, ±20 % jitter, never below 500 ms
pub fn backoff_ms(attempt: u32, rnd: f64) -> u64 {
    let base = 1000u64 << attempt.min(6);
    let capped = base.min(60_000) as f64;
    let jitter = capped * 0.2 * (rnd * 2.0 - 1.0);
    ((capped + jitter) as u64).max(500)
}

/// lower-case extension
pub fn ext(name: &str) -> String {
    match name.rfind('.') {
        Some(d) => name[d + 1..].to_ascii_lowercase(),
        None => String::new(),
    }
}

/// kind by extension (1.x PeekFs lists); None = not shared
pub fn kind(name: &str) -> Option<&'static str> {
    match ext(name).as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "bmp" | "heic" | "heif" | "jfif" => {
            Some("image")
        }
        "mp4" | "m4v" | "mov" | "webm" | "mkv" | "3gp" | "ts" | "avi" => Some("video"),
        "mp3" | "m4a" | "aac" | "ogg" | "oga" | "opus" | "wav" | "flac" => Some("audio"),
        "zip" => Some("archive"),
        "ttf" | "otf" | "woff" | "woff2" => Some("font"),
        _ => None,
    }
}

/// dot-files and the PeekAround trash are never listed
pub fn hidden(name: &str) -> bool {
    name.starts_with('.') || name == ".peekaround-trash"
}

/// MIME type by extension
pub fn mime(name: &str) -> &'static str {
    match ext(name).as_str() {
        "jpg" | "jpeg" | "jfif" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "heic" | "heif" => "image/heif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "3gp" => "video/3gpp",
        "ts" => "video/mp2t",
        "avi" => "video/x-msvideo",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "zip" => "application/zip",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_frames() {
        assert_eq!(
            parse(r#"{"t":"pending","code":"4821"}"#),
            Incoming::Pending("4821".into())
        );
        assert_eq!(
            parse(r#"{"t":"paired","token":"abc"}"#),
            Incoming::Paired("abc".into())
        );
        assert_eq!(parse(r#"{"t":"paired"}"#), Incoming::Unknown);
        assert_eq!(
            parse(r#"{"t":"welcome","server":"PC","paired":true}"#),
            Incoming::Welcome("PC".into())
        );
        match parse(
            r#"{"t":"req","rid":"AAAAAAAAAAAAAAAAAAAAAA","op":"list","args":{"key":"k1","dir":"","recursive":true}}"#,
        ) {
            Incoming::Req { rid, op, args } => {
                assert_eq!(rid, "AAAAAAAAAAAAAAAAAAAAAA");
                assert_eq!(op, "list");
                assert_eq!(args["key"], "k1");
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(parse(r#"{"t":"req","rid":"x","op":"list"}"#), Incoming::Unknown);
        assert_eq!(
            parse(r#"{"t":"cancel","rid":"AAAAAAAAAAAAAAAAAAAAAA"}"#),
            Incoming::Cancel("AAAAAAAAAAAAAAAAAAAAAA".into())
        );
        assert_eq!(parse("nope"), Incoming::Unknown);
    }

    #[test]
    fn list_lines_match_the_golden_transcript() {
        // crates/pa-link/tests/fixtures/link/list-and-thumb.json
        let a = list_line(
            "IMG_1.jpg",
            "image",
            2048,
            1_700_000_000_000,
            &LineMeta {
                w: Some(4000),
                h: Some(3000),
                d: None,
                t: Some(1_699_999_999_000),
                id: Some(11),
            },
        );
        assert_eq!(
            a,
            r#"{"p":"IMG_1.jpg","k":"image","s":2048,"m":1700000000000,"w":4000,"h":3000,"d":null,"t":1699999999000,"id":11}"#
        );
        let b = list_line(
            "Trip/VID_2.mp4",
            "video",
            4096,
            1_700_000_500_000,
            &LineMeta {
                w: Some(1920),
                h: Some(1080),
                d: Some(12.5),
                t: Some(1_700_000_400_000),
                id: Some(12),
            },
        );
        assert_eq!(
            b,
            r#"{"p":"Trip/VID_2.mp4","k":"video","s":4096,"m":1700000500000,"w":1920,"h":1080,"d":12.5,"t":1700000400000,"id":12}"#
        );
        assert_eq!(dir_line("Empty"), r#"{"dir":"Empty"}"#);
        assert_eq!(
            list_line("a\"b.jpg", "image", 1, 2, &LineMeta::default()),
            r#"{"p":"a\"b.jpg","k":"image","s":1,"m":2,"w":null,"h":null,"d":null,"t":null,"id":null}"#
        );
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range(None, 100), ByteRange::Full);
        assert_eq!(parse_range(Some("bytes=0-9"), 100), ByteRange::Part(0, 9));
        assert_eq!(parse_range(Some("bytes=90-"), 100), ByteRange::Part(90, 99));
        assert_eq!(parse_range(Some("bytes=-10"), 100), ByteRange::Part(90, 99));
        assert_eq!(parse_range(Some("bytes=-500"), 100), ByteRange::Part(0, 99));
        assert_eq!(parse_range(Some("bytes=50-1000"), 100), ByteRange::Part(50, 99));
        assert_eq!(parse_range(Some("bytes=100-"), 100), ByteRange::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=9-3"), 100), ByteRange::Full);
        assert_eq!(parse_range(Some("items=0-1"), 100), ByteRange::Full);
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 100), ByteRange::Part(0, 1));
        let r = file_reply(Some("bytes=0-1048575"), 4_000_000, 7, "video/mp4");
        assert_eq!(r.status, 206);
        assert_eq!(r.length, 1_048_576);
        assert!(r
            .headers
            .contains(&("X-Peek-Content-Range".into(), "bytes 0-1048575/4000000".into())));
        let r = file_reply(Some("bytes=5000000-"), 4_000_000, 7, "video/mp4");
        assert_eq!((r.status, r.length), (416, 0));
    }

    #[test]
    fn path_guard() {
        assert_eq!(clean_rel("").unwrap(), "");
        assert_eq!(clean_rel("a/b/").unwrap(), "a/b");
        for bad in ["/etc", "../x", "a/../b", "a//b", "a\\b", "C:/x", "a/./b", "x\0"] {
            assert_eq!(clean_rel(bad).unwrap_err().status, 403, "{bad}");
        }
        let d = std::env::temp_dir().join(format!("pa-phone-guard-{}", std::process::id()));
        std::fs::create_dir_all(d.join("root/sub")).unwrap();
        std::fs::write(d.join("root/sub/a.jpg"), b"x").unwrap();
        std::fs::write(d.join("secret.txt"), b"x").unwrap();
        let root = d.join("root");
        let f = resolve(&root, "sub/a.jpg").unwrap();
        assert!(f.ends_with("a.jpg"));
        assert_eq!(rel_of(&root.canonicalize().unwrap(), &f), "sub/a.jpg");
        assert_eq!(resolve(&root, "../secret.txt").unwrap_err().status, 403);
        assert_eq!(resolve(&root, "nope.jpg").unwrap_err().status, 404);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_and_backoff() {
        assert_eq!(clean_name(" IMG.jpg ").unwrap(), "IMG.jpg");
        for bad in ["", ".", "..", "a/b", "a\\b", "a\nb"] {
            assert!(clean_name(bad).is_err(), "{bad:?}");
        }
        let taken = ["IMG_1.jpg", "IMG_1 (1).jpg"];
        assert_eq!(
            unique_name("IMG_1.jpg", |n| taken.contains(&n)),
            "IMG_1 (2).jpg"
        );
        assert_eq!(unique_name(".nomedia", |n| n == ".nomedia"), ".nomedia (1)");
        assert_eq!(backoff_ms(0, 0.5), 1000);
        assert_eq!(backoff_ms(3, 0.5), 8000);
        assert_eq!(backoff_ms(20, 0.5), 60_000);
        assert_eq!(backoff_ms(0, 0.0), 800);
        assert_eq!(kind("A.HEIC"), Some("image"));
        assert_eq!(kind("x.txt"), None);
        assert_eq!(mime("v.mov"), "video/quicktime");
    }
}
