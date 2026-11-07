//! The ffmpeg / ffprobe command-line wrapper (mirrors `packages/server/src/ffmpeg.ts`): availability
//! check, `ffprobe -show_format -show_streams` JSON and — for installs without ffprobe (the desktop
//! app ships none) — a parser of the input description `ffmpeg -hide_banner -i` prints on stderr.

use crate::error::{MediaError, Result};
use crate::process::{self, Priority};
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

/// Container + stream facts of a video/audio file (same fields and JSON shape as the TS
/// `ProbeResult`). TODO(unify): replace with `pa_core::ProbeResult`, which now exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub duration: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// 0 / 90 / 180 / 270 (clockwise display rotation)
    pub rotation: u32,
    pub codec: Option<String>,
    pub audio_codec: Option<String>,
    pub fps: Option<f64>,
    /// bits per second
    pub bitrate: Option<f64>,
    pub container: Option<String>,
    /// ms since the epoch
    pub creation_time: Option<f64>,
    pub has_audio: bool,
    /// container + video stream tags, lower-cased keys (location, com.apple.quicktime.*, com.android.*); container wins
    pub tags: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProbeOptions {
    /// HTTP headers for URL inputs (ffmpeg `-headers`, CRLF terminated lines)
    pub headers: Option<String>,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub timeout: Option<Duration>,
    pub input: Option<Vec<u8>>,
}

/// ffmpeg + (optional) ffprobe binaries. Cheap to share (`Arc<Ffmpeg>`); the flags are atomics.
#[derive(Debug)]
pub struct Ffmpeg {
    pub ffmpeg_path: PathBuf,
    pub ffprobe_path: Option<PathBuf>,
    available: AtomicBool,
    probe_available: AtomicBool,
    decode_threads: AtomicU32,
    encoders: tokio::sync::OnceCell<HashSet<String>>,
}

impl Ffmpeg {
    pub fn new(ffmpeg: impl Into<PathBuf>, ffprobe: Option<PathBuf>) -> Self {
        Self {
            ffmpeg_path: ffmpeg.into(),
            ffprobe_path: ffprobe.filter(|p| !p.as_os_str().is_empty()),
            available: AtomicBool::new(true),
            probe_available: AtomicBool::new(true),
            decode_threads: AtomicU32::new(0),
            encoders: tokio::sync::OnceCell::new(),
        }
    }

    /// `ffmpeg -version` / `ffprobe -version` (10 s timeout each).
    pub async fn check(&self) {
        let ok = process::can_run(&self.ffmpeg_path, &["-version"]).await;
        self.available.store(ok, Ordering::Relaxed);
        let probe = match &self.ffprobe_path {
            Some(p) => process::can_run(p, &["-version"]).await,
            None => false,
        };
        self.probe_available.store(probe, Ordering::Relaxed);
    }

    pub fn available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }
    pub fn set_available(&self, v: bool) {
        self.available.store(v, Ordering::Relaxed)
    }
    /// a working ffprobe; otherwise [`Ffmpeg::probe`] parses `ffmpeg -i`
    pub fn probe_available(&self) -> bool {
        self.probe_available.load(Ordering::Relaxed)
    }
    pub fn set_probe_available(&self, v: bool) {
        self.probe_available.store(v, Ordering::Relaxed)
    }
    /// decoder threads for thumbnail frames (Settings.indexIntensity); 0 = ffmpeg's default
    pub fn decode_threads(&self) -> u32 {
        self.decode_threads.load(Ordering::Relaxed)
    }
    pub fn set_decode_threads(&self, n: u32) {
        self.decode_threads.store(n, Ordering::Relaxed)
    }

    /// Run ffmpeg (`-hide_banner -loglevel error` + args) at low priority, collecting stdout.
    pub async fn run<S: AsRef<str>>(&self, args: &[S], opts: RunOptions) -> Result<Vec<u8>> {
        let mut a: Vec<&str> = vec!["-hide_banner", "-loglevel", "error"];
        a.extend(args.iter().map(|s| s.as_ref()));
        process::run_collect(
            &self.ffmpeg_path,
            &a,
            opts.timeout,
            opts.input,
            Priority::Low,
        )
        .await
    }

    /// A long-running ffmpeg (`-hide_banner` + args) with piped stdio, normal priority (jobs).
    pub fn spawn<S: AsRef<str>>(&self, args: &[S]) -> std::io::Result<tokio::process::Child> {
        let mut c = process::command(&self.ffmpeg_path, Priority::Normal);
        c.arg("-hide_banner")
            .args(args.iter().map(|s| s.as_ref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c.spawn()
    }

    /// Container + stream info: ffprobe when available, else parsed from `ffmpeg -i`.
    pub async fn probe(&self, file: &str, opts: &ProbeOptions) -> Option<ProbeResult> {
        if self.probe_available() {
            return self.ffprobe(file, opts).await;
        }
        if !self.available() {
            return None;
        }
        let mut args: Vec<&str> = vec!["-hide_banner"];
        if let Some(h) = &opts.headers {
            args.extend(["-headers", h]);
        }
        args.extend(["-i", file]);
        let stderr = process::run_stderr(
            &self.ffmpeg_path,
            &args,
            opts.timeout.unwrap_or(Duration::from_secs(20)),
        )
        .await
        .ok()?;
        parse_ffmpeg_info(&stderr)
    }

    pub async fn ffprobe(&self, file: &str, opts: &ProbeOptions) -> Option<ProbeResult> {
        let bin = self.ffprobe_path.as_ref()?;
        let mut args: Vec<&str> = vec!["-v", "quiet"];
        if let Some(h) = &opts.headers {
            args.extend(["-headers", h]);
        }
        args.extend([
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            file,
        ]);
        let out = process::run_collect(
            bin,
            &args,
            Some(opts.timeout.unwrap_or(Duration::from_secs(20))),
            None,
            Priority::Low,
        )
        .await
        .ok()?;
        let j: Value = serde_json::from_slice(&out).ok()?;
        Some(probe_from_json(&j))
    }

    /// Arguments of [`Ffmpeg::frame`] (without the global `-hide_banner -loglevel error`).
    pub fn frame_args(&self, file: &str, at_seconds: f64, max_width: u32) -> Vec<String> {
        let mut a = Vec::new();
        let t = self.decode_threads();
        if t > 0 {
            a.extend(["-threads".to_string(), t.to_string()]);
        }
        if at_seconds > 0.0 {
            a.extend(["-ss".to_string(), format!("{at_seconds:.3}")]);
        }
        a.extend(
            [
                "-i",
                file,
                "-frames:v",
                "1",
                "-vf",
                &format!("scale='min({max_width},iw)':-2"),
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "pipe:1",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        a
    }

    /// One frame as PNG (30 s timeout); an empty result at `at > 0` retries at 0.
    pub async fn frame(&self, file: &str, at_seconds: f64, max_width: u32) -> Result<Vec<u8>> {
        let buf = self
            .run(
                &self.frame_args(file, at_seconds, max_width),
                RunOptions {
                    timeout: Some(Duration::from_secs(30)),
                    input: None,
                },
            )
            .await?;
        if buf.is_empty() && at_seconds > 0.0 {
            return Box::pin(self.frame(file, 0.0, max_width)).await;
        }
        Ok(buf)
    }

    /// Encoder names of this build (`ffmpeg -encoders`, cached; empty when it fails).
    pub async fn encoders(&self) -> &HashSet<String> {
        self.encoders
            .get_or_init(|| async {
                match self
                    .run(
                        &["-encoders"],
                        RunOptions {
                            timeout: Some(Duration::from_secs(15)),
                            input: None,
                        },
                    )
                    .await
                {
                    Ok(b) => parse_encoders(&String::from_utf8_lossy(&b)),
                    Err(_) => HashSet::new(),
                }
            })
            .await
    }
}

/// `ffmpeg -encoders` output → encoder names
pub fn parse_encoders(text: &str) -> HashSet<String> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*([VAS][A-Z.]{5})\s+(\S+)").unwrap());
    text.lines()
        .filter_map(|l| RE.captures(l))
        .map(|c| c[2].to_string())
        .filter(|n| n != "=")
        .collect()
}

/// `Math.round` (half towards +∞)
pub(crate) fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// JavaScript `Number(string)` for the inputs ffmpeg produces (trimmed decimal / empty = 0 / NaN).
pub(crate) fn js_number(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    if t.chars()
        .any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
    {
        return if t == "Infinity" || t == "+Infinity" {
            f64::INFINITY
        } else if t == "-Infinity" {
            f64::NEG_INFINITY
        } else {
            f64::NAN
        };
    }
    t.parse().unwrap_or(f64::NAN)
}

fn value_number(v: Option<&Value>) -> f64 {
    match v {
        None | Some(Value::Null) => f64::NAN,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => js_number(s),
        Some(Value::Bool(b)) => *b as u8 as f64,
        Some(_) => f64::NAN,
    }
}

/// round to a multiple of 90 in 0..360
pub fn norm_rotation(r: f64) -> u32 {
    let r = if r.is_finite() { r } else { 0.0 };
    let q = js_round(r / 90.0) * 90.0;
    (((q % 360.0) + 360.0) % 360.0) as u32
}

fn fps_of(s: Option<&str>) -> Option<f64> {
    let s = s?;
    let (n, d) = s.split_once('/')?;
    let (n, d) = (js_number(n), js_number(d.split('/').next().unwrap_or("")));
    if d == 0.0 || d.is_nan() {
        return None;
    }
    let v = js_round(n / d * 100.0) / 100.0;
    v.is_finite().then_some(v)
}

/// `Date.parse` for the timestamps containers carry (ISO 8601; no offset = local time, a date alone = UTC).
pub fn js_date_parse(s: &str) -> Option<f64> {
    use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};
    let t = s.trim();
    if let Ok(d) = DateTime::parse_from_rfc3339(t) {
        return Some(d.timestamp_millis() as f64);
    }
    for f in [
        "%Y-%m-%dT%H:%M:%S%.f%#z",
        "%Y-%m-%dT%H:%M%#z",
        "%Y-%m-%d %H:%M:%S%.f%#z",
    ] {
        if let Ok(d) = DateTime::parse_from_str(t, f) {
            return Some(d.timestamp_millis() as f64);
        }
    }
    if let Some(z) = t.strip_suffix('Z').or_else(|| t.strip_suffix('z')) {
        for f in [
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%d %H:%M:%S%.f",
        ] {
            if let Ok(d) = NaiveDateTime::parse_from_str(z, f) {
                return Some(d.and_utc().timestamp_millis() as f64);
            }
        }
    }
    for f in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y/%m/%d %H:%M:%S",
    ] {
        if let Ok(d) = NaiveDateTime::parse_from_str(t, f) {
            return Local
                .from_local_datetime(&d)
                .earliest()
                .map(|d| d.timestamp_millis() as f64);
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis() as f64);
    }
    None
}

fn creation_of(ct: Option<&str>) -> Option<f64> {
    js_date_parse(ct?).filter(|t| t.is_finite() && *t > 0.0)
}

fn value_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

/// ffprobe `-show_format -show_streams` JSON → ProbeResult
pub fn probe_from_json(j: &Value) -> ProbeResult {
    let empty = Vec::new();
    let streams = j.get("streams").and_then(Value::as_array).unwrap_or(&empty);
    let truthy = |v: Option<&Value>| match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    };
    let v = streams.iter().find(|s| {
        s.get("codec_type").and_then(Value::as_str) == Some("video")
            && !truthy(s.get("disposition").and_then(|d| d.get("attached_pic")))
    });
    let a = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(Value::as_str) == Some("audio"));
    let format = j.get("format");
    let mut rotation = 0;
    if let Some(v) = v {
        let sd = v
            .get("side_data_list")
            .and_then(Value::as_array)
            .and_then(|l| l.iter().find(|d| d.get("rotation").is_some()));
        let r = match sd.and_then(|d| d.get("rotation")) {
            Some(r) => value_number(Some(r)),
            None => match v.get("tags").and_then(|t| t.get("rotate")) {
                Some(r) => value_number(Some(r)),
                None => 0.0,
            },
        };
        rotation = norm_rotation(r);
    }
    let dim = |k: &str| {
        v.map(|v| value_number(v.get(k)))
            .filter(|x| x.is_finite() && *x != 0.0)
            .map(|x| x as u32)
    };
    let (mut width, mut height) = (dim("width"), dim("height"));
    if width.is_some() && height.is_some() && (rotation == 90 || rotation == 270) {
        std::mem::swap(&mut width, &mut height);
    }
    let fps = fps_of(
        v.and_then(|v| v.get("avg_frame_rate"))
            .and_then(Value::as_str),
    )
    .or_else(|| {
        fps_of(
            v.and_then(|v| v.get("r_frame_rate"))
                .and_then(Value::as_str),
        )
    });
    let mut tags = IndexMap::new();
    for src in [
        v.and_then(|v| v.get("tags")),
        format.and_then(|f| f.get("tags")),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(o) = src.as_object() {
            for (k, val) in o {
                if let Some(s) = value_string(val) {
                    tags.insert(k.to_lowercase(), s);
                }
            }
        }
    }
    let ct = format
        .and_then(|f| f.get("tags"))
        .and_then(|t| t.get("creation_time"))
        .filter(|x| !x.is_null())
        .or_else(|| {
            v.and_then(|v| v.get("tags"))
                .and_then(|t| t.get("creation_time"))
        });
    let ct = ct.and_then(value_string);
    fn pick(o: Option<&Value>) -> Option<&Value> {
        o.and_then(|o| o.get("duration")).filter(|x| !x.is_null())
    }
    let duration = value_number(pick(format).or_else(|| pick(v)).or_else(|| pick(a)));
    let bitrate = value_number(format.and_then(|f| f.get("bit_rate")));
    ProbeResult {
        duration: duration.is_finite().then_some(duration),
        width,
        height,
        rotation,
        codec: v.and_then(|v| v.get("codec_name")).and_then(value_string),
        audio_codec: a.and_then(|a| a.get("codec_name")).and_then(value_string),
        fps,
        bitrate: (bitrate.is_finite() && bitrate != 0.0).then_some(bitrate),
        container: format
            .and_then(|f| f.get("format_name"))
            .and_then(value_string),
        creation_time: creation_of(ct.as_deref()),
        has_audio: a.is_some(),
        tags,
    }
}

struct ParsedStream {
    kind: String,
    desc: String,
    tags: IndexMap<String, String>,
    rotation: Option<f64>,
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
    };
}

/// `WxH` in a stream description: digits (2–5) after start / space / comma, followed by end,
/// space, comma or `[` (the TS regex uses a lookahead, unsupported by the regex crate).
fn find_wh(desc: &str) -> Option<(u32, u32)> {
    let b = desc.as_bytes();
    for start in 0..b.len() {
        if start > 0 && b[start - 1] != b' ' && b[start - 1] != b',' {
            continue;
        }
        let run = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
        let n1 = run(start);
        if !(2..=5).contains(&n1) || b.get(start + n1) != Some(&b'x') {
            continue;
        }
        let s2 = start + n1 + 1;
        let n2 = run(s2);
        if !(2..=5).contains(&n2) {
            continue;
        }
        match b.get(s2 + n2) {
            None | Some(b' ') | Some(b',') | Some(b'[') => {}
            _ => continue,
        }
        return Some((
            desc[start..start + n1].parse().ok()?,
            desc[s2..s2 + n2].parse().ok()?,
        ));
    }
    None
}

/// Parse the input description `ffmpeg -hide_banner -i <file>` prints on stderr (ffmpeg 4–7).
/// Mirrors [`probe_from_json`]; None when there is no "Input #0" (not a media file / unreadable).
pub fn parse_ffmpeg_info(stderr: &str) -> Option<ProbeResult> {
    re!(INPUT0, r"^Input #0, ");
    re!(CONTAINER, r"^Input #0, (.+?), from '");
    re!(
        STOP,
        r"^(Input #[1-9]|Output #|Stream mapping:|At least one output)"
    );
    re!(CONT, r"^: ?(.*)$");
    re!(KV, r"^([^:]+?)\s*: ?(.*)$");
    re!(ROT, r"rotation of (-?[\d.]+) degrees");
    re!(
        DUR,
        r"^Duration: (N/A|(\d+):(\d+):(\d+(?:\.\d+)?))(?:, start: [-\d.]+)?, bitrate: (N/A|(\d+) kb/s)"
    );
    re!(
        STREAM,
        r"^Stream #0:\d+(?:\[[^\]]*\])?(?:\(([^)]*)\))?: (\w+): (.*)$"
    );
    re!(CODEC, r"^([\w-]+)");
    re!(FPS, r", ([\d.]+)(k?) fps\b");
    re!(TBR, r", ([\d.]+)(k?) tbr\b");

    let lines: Vec<&str> = stderr
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let at = lines.iter().position(|l| INPUT0.is_match(l))?;
    let container = CONTAINER.captures(lines[at]).map(|c| c[1].to_string());
    let mut format: IndexMap<String, String> = IndexMap::new();
    let mut streams: Vec<ParsedStream> = Vec::new();
    let mut duration = None;
    let mut bitrate = None;
    // where "key : value" lines go: None, Some(usize::MAX) = format, Some(i) = stream i
    let mut target: Option<usize> = None;
    let mut target_indent = 0usize;
    let mut side_data: Option<usize> = None;
    let mut last_key: Option<String> = None;
    const FORMAT: usize = usize::MAX;
    for line in &lines[at + 1..] {
        let line: &str = line;
        if STOP.is_match(line) || (!line.starts_with(' ') && !line.trim().is_empty()) {
            break;
        }
        let trimmed_start = line.trim_start();
        let indent = line.len() - trimmed_start.len();
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(tg) = target {
            if indent > target_indent {
                let map = if tg == FORMAT {
                    &mut format
                } else {
                    &mut streams[tg].tags
                };
                if let (Some(c), Some(k)) = (CONT.captures(trimmed_start), last_key.as_ref()) {
                    if let Some(v) = map.get_mut(k) {
                        v.push('\n');
                        v.push_str(&c[1]);
                    }
                    continue;
                }
                if let Some(c) = KV.captures(trimmed_start) {
                    let k = c[1].trim().to_lowercase();
                    map.insert(k.clone(), c[2].to_string());
                    last_key = Some(k);
                    continue;
                }
            }
        }
        target = None;
        last_key = None;
        if let Some(sd) = side_data {
            if indent > 4 {
                if let Some(c) = ROT.captures(t) {
                    streams[sd].rotation = Some(js_number(&c[1]));
                }
                continue;
            }
        }
        side_data = None;
        if t == "Metadata:" {
            target = if indent <= 2 {
                Some(FORMAT)
            } else {
                streams.len().checked_sub(1)
            };
            target_indent = indent;
            continue;
        }
        if t == "Side data:" {
            side_data = streams.len().checked_sub(1);
            continue;
        }
        if let Some(d) = DUR.captures(t) {
            if let (Some(h), Some(m), Some(s)) = (d.get(2), d.get(3), d.get(4)) {
                duration = Some(
                    js_number(h.as_str()) * 3600.0
                        + js_number(m.as_str()) * 60.0
                        + js_number(s.as_str()),
                );
            }
            if let Some(b) = d.get(6) {
                bitrate = Some(js_number(b.as_str()) * 1000.0);
            }
            continue;
        }
        if let Some(st) = STREAM.captures(t) {
            let mut tags = IndexMap::new();
            if let Some(lang) = st.get(1) {
                tags.insert("language".to_string(), lang.as_str().to_string());
            }
            streams.push(ParsedStream {
                kind: st[2].to_lowercase(),
                desc: st[3].to_string(),
                tags,
                rotation: None,
            });
        }
    }
    let v = streams
        .iter()
        .find(|s| s.kind == "video" && !s.desc.contains("(attached pic)"));
    let a = streams.iter().find(|s| s.kind == "audio");
    let codec_of = |s: Option<&ParsedStream>| {
        s.and_then(|s| CODEC.captures(&s.desc))
            .map(|c| c[1].to_string())
    };
    let (mut width, mut height, mut fps, mut rotation) = (None, None, None, 0);
    if let Some(v) = v {
        if let Some((w, h)) = find_wh(&v.desc) {
            width = Some(w);
            height = Some(h);
        }
        if let Some(c) = FPS.captures(&v.desc).or_else(|| TBR.captures(&v.desc)) {
            let k = if c[2].is_empty() { 1.0 } else { 1000.0 };
            fps = Some(js_round(js_number(&c[1]) * k * 100.0) / 100.0);
        }
        let r = match v.rotation {
            Some(r) => r,
            None => v.tags.get("rotate").map(|s| js_number(s)).unwrap_or(0.0),
        };
        rotation = norm_rotation(r);
    }
    if width.is_some() && height.is_some() && (rotation == 90 || rotation == 270) {
        std::mem::swap(&mut width, &mut height);
    }
    let mut tags = IndexMap::new();
    for src in [v.map(|v| &v.tags), Some(&format)].into_iter().flatten() {
        for (k, val) in src {
            tags.insert(k.clone(), val.clone());
        }
    }
    let ct = format
        .get("creation_time")
        .or_else(|| v.and_then(|v| v.tags.get("creation_time")));
    Some(ProbeResult {
        duration,
        width,
        height,
        rotation,
        codec: codec_of(v),
        audio_codec: codec_of(a),
        fps,
        bitrate,
        container,
        creation_time: creation_of(ct.map(|s| s.as_str())),
        has_audio: a.is_some(),
        tags,
    })
}

/// Video codecs + containers every modern browser can play natively.
pub fn is_browser_playable(ext: &str, codec: Option<&str>) -> bool {
    if !["mp4", "m4v", "mov", "webm", "ogv"].contains(&ext) {
        return false;
    }
    match codec {
        None => ext != "mov",
        Some(c) => ["h264", "vp8", "vp9", "av1", "theora"].contains(&c),
    }
}

/// Where a video's thumbnail frame is taken: 10 % in, at most 3 s (1 s when the duration is unknown).
pub fn thumb_frame_time(duration: Option<f64>) -> f64 {
    match duration {
        Some(d) if d > 0.0 => (d * 0.1).min(3.0),
        _ => 1.0,
    }
}

impl From<serde_json::Error> for MediaError {
    fn from(e: serde_json::Error) -> Self {
        MediaError::Decode(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wh_lookahead_port() {
        assert_eq!(
            find_wh("h264 (High), yuv420p(progressive), 720x1280 [SAR 1:1 DAR 9:16], 30 fps"),
            Some((720, 1280))
        );
        assert_eq!(
            find_wh("mjpeg, yuvj420p(pc), 320x240, 90k tbr"),
            Some((320, 240))
        );
        assert_eq!(find_wh("h264, 1280x720"), Some((1280, 720)));
        assert_eq!(find_wh("h264, 0x31637661), 640x360"), Some((640, 360)));
        assert_eq!(find_wh("h264 1x1"), None);
    }

    #[test]
    fn rotation_and_fps() {
        assert_eq!(norm_rotation(-90.0), 270);
        assert_eq!(norm_rotation(450.0), 90);
        assert_eq!(norm_rotation(f64::NAN), 0);
        assert_eq!(fps_of(Some("30000/1001")), Some(29.97));
        assert_eq!(fps_of(Some("0/0")), None);
        assert_eq!(
            js_date_parse("2023-05-01T10:00:00.000000Z"),
            Some(1682935200000.0)
        );
        assert_eq!(
            js_date_parse("2024-07-03T21:00:00+0300"),
            Some(1720029600000.0)
        );
        assert!(is_browser_playable("mp4", Some("h264")));
        assert!(!is_browser_playable("mov", None));
        assert!(!is_browser_playable("mkv", Some("h264")));
    }

    #[test]
    fn encoders() {
        let text = "Encoders:\n V..... = Video\n A..... = Audio\n ------\n V....D libx264              libx264 H.264\n V....D libvpx-vp9           libvpx VP9\n A....D aac                  AAC\n A....D libopus              libopus Opus\n";
        let mut v: Vec<_> = parse_encoders(text).into_iter().collect();
        v.sort();
        assert_eq!(v, ["aac", "libopus", "libvpx-vp9", "libx264"]);
    }
}
