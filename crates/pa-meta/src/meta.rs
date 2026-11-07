//! Metadata extraction → grouping columns (mirrors `packages/server/src/media/metadata.ts`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pa_core::{js, GeoIndex, MediaKind, Playback, ProbeResult};
use serde::{Deserialize, Serialize};

use crate::exifr::{self, Mode};
use crate::source::{ReaderSource, Source};
use crate::value::{get, Tags, Val};

/// Version of the grouping metadata; rows with `meta_v < META_V` are re-read by the enrichment loop.
pub const META_V: i64 = 2;

/// `time_src` column values.
pub mod time_src {
    /// file modification time
    pub const MTIME: u8 = 0;
    /// EXIF wall clock, no offset (home zone)
    pub const LOCAL: u8 = 1;
    /// EXIF wall clock + offset
    pub const LOCAL_OFFSET: u8 = 2;
    /// true instant (video creation_time, Takeout)
    pub const INSTANT: u8 = 3;
}

/// Video stream details stored in the `extra` column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoExtra {
    /// video codec
    pub codec: Option<String>,
    /// audio codec
    pub audio_codec: Option<String>,
    /// frames per second
    #[serde(with = "js::num_opt")]
    pub fps: Option<f64>,
    /// bits per second
    #[serde(with = "js::num_opt")]
    pub bitrate: Option<f64>,
    /// degrees
    pub rotation: i32,
    /// has an audio stream
    pub has_audio: bool,
}

/// Everything read from one file (`ExtractedMeta`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ExtractedMeta {
    /// px
    #[serde(with = "js::num_opt")]
    pub width: Option<f64>,
    /// px
    #[serde(with = "js::num_opt")]
    pub height: Option<f64>,
    /// seconds
    #[serde(with = "js::num_opt")]
    pub duration: Option<f64>,
    /// how clients play the original
    pub playback: Playback,
    /// video details (`extra` column, JSON)
    pub extra: Option<VideoExtra>,
    /// embedded wall clock (EXIF / QuickTime), ms as UTC
    pub local: Option<i64>,
    /// offset of `local`, minutes east
    pub offset: Option<i32>,
    /// embedded true instant (video creation_time / Takeout)
    pub instant: Option<i64>,
    /// latitude
    #[serde(with = "js::num_opt")]
    pub lat: Option<f64>,
    /// longitude
    #[serde(with = "js::num_opt")]
    pub lon: Option<f64>,
    /// raw make
    pub make: Option<String>,
    /// raw model
    pub model: Option<String>,
    /// lens model
    pub lens: Option<String>,
    /// XMP GPano panorama
    pub gpano: bool,
    /// frames / pages
    pub pages: Option<u32>,
}

/// Columns written for the grouping metadata (`GroupColumns`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupColumns {
    /// true instant
    pub taken_at: i64,
    /// wall clock as UTC
    pub taken_local: i64,
    /// minutes east when known
    pub tz_off: Option<i32>,
    /// [`time_src`]
    pub time_src: u8,
    /// latitude
    #[serde(with = "js::num_opt")]
    pub lat: Option<f64>,
    /// longitude
    #[serde(with = "js::num_opt")]
    pub lon: Option<f64>,
    /// GeoNames row
    pub geo_id: Option<i64>,
    /// country
    pub cc: Option<String>,
    /// admin1 code
    pub admin1: Option<String>,
    /// city name
    pub city: Option<String>,
    /// normalised make
    pub make: Option<String>,
    /// normalised model
    pub model: Option<String>,
    /// lens
    pub lens: Option<String>,
    /// MEDIA_FLAG bits
    pub flags: u32,
}

/// `UPDATE media SET …` fragment for [`group_params`].
pub const GROUP_SET_SQL: &str =
    "taken_at = ?, taken_local = ?, tz_off = ?, time_src = ?, lat = ?, lon = ?, geo_id = ?, cc = ?, admin1 = ?, city = ?, make = ?, model = ?, lens = ?, flags = ?";

/// One SQL parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    /// NULL
    Null,
    /// INTEGER
    Int(i64),
    /// REAL
    Real(f64),
    /// TEXT
    Text(String),
}

/// Parameters for [`GROUP_SET_SQL`] (same order).
pub fn group_params(g: &GroupColumns) -> Vec<SqlValue> {
    let int = |v: Option<i64>| v.map(SqlValue::Int).unwrap_or(SqlValue::Null);
    let real = |v: Option<f64>| v.map(SqlValue::Real).unwrap_or(SqlValue::Null);
    let text = |v: &Option<String>| v.clone().map(SqlValue::Text).unwrap_or(SqlValue::Null);
    vec![
        SqlValue::Int(g.taken_at),
        SqlValue::Int(g.taken_local),
        int(g.tz_off.map(i64::from)),
        SqlValue::Int(g.time_src as i64),
        real(g.lat),
        real(g.lon),
        int(g.geo_id),
        text(&g.cc),
        text(&g.admin1),
        text(&g.city),
        text(&g.make),
        text(&g.model),
        text(&g.lens),
        SqlValue::Int(g.flags as i64),
    ]
}

const DATE_TAGS: [&str; 5] = [
    "DateTimeOriginal",
    "CreateDate",
    "DateTimeDigitized",
    "ModifyDate",
    "DateTime",
];

fn offset_tags(tag: &str) -> &'static [&'static str] {
    match tag {
        "DateTimeOriginal" => &["OffsetTimeOriginal", "OffsetTime"],
        "CreateDate" | "DateTimeDigitized" => &["OffsetTimeDigitized", "OffsetTime"],
        _ => &["OffsetTime"],
    }
}

/// still formats that can never be animated: their pixel header is skipped when the size is known
const SINGLE_FRAME: [&str; 6] = ["jpg", "jpeg", "jpe", "jfif", "bmp", "ico"];

fn gps_value(v: &Val) -> pa_core::GpsValue<'_> {
    match v {
        Val::Num(n) => pa_core::GpsValue::Num(*n),
        Val::Str(s) => pa_core::GpsValue::Text(s),
        _ => pa_core::GpsValue::Other,
    }
}

/// `applyExif`: date / offset / size / GPS / camera / GPano from exifr's merged output.
pub fn apply_exif(m: &mut ExtractedMeta, ex: &Tags) {
    for t in DATE_TAGS {
        let Some(r) = get(ex, t)
            .as_str()
            .and_then(pa_core::parse_date_time_with_offset)
        else {
            continue;
        };
        m.local = Some(r.local);
        m.offset = r.offset.or_else(|| {
            offset_tags(t)
                .iter()
                .find_map(|k| get(ex, k).as_str().and_then(pa_core::parse_offset))
        });
        break;
    }
    if m.width.is_none_or(|w| w == 0.0 || w.is_nan()) {
        let first = |keys: [&str; 3]| {
            keys.iter()
                .map(|k| get(ex, k))
                .find(|v| !v.is_undef())
                .cloned()
                .unwrap_or_default()
        };
        let or0 = |v: f64| if v.is_nan() { 0.0 } else { v };
        let w = or0(first(["ExifImageWidth", "ImageWidth", "PixelXDimension"]).number());
        let h = or0(first(["ExifImageHeight", "ImageHeight", "PixelYDimension"]).number());
        if w != 0.0 && h != 0.0 {
            let swap = get(ex, "Orientation").number() >= 5.0;
            m.width = Some(if swap { h } else { w });
            m.height = Some(if swap { w } else { h });
        }
    }
    let mut lat = get(ex, "latitude").number();
    let mut lon = get(ex, "longitude").number();
    if !pa_core::is_valid_gps(lat, lon) {
        // XMP (Lightroom, Photos exports, some phone apps): exif:GPSLatitude="41,0.738N"
        let parts = |v: &Val| -> Vec<f64> {
            match v {
                Val::Arr(a) => a.iter().map(Val::number).collect(),
                _ => vec![],
            }
        };
        let (la, lo) = (get(ex, "GPSLatitude"), get(ex, "GPSLongitude"));
        let (pla, plo) = (parts(la), parts(lo));
        let ref_of = |name: &str| -> Option<&str> { get(ex, name).as_str() };
        let one = |v: &Val, p: &[f64], r: Option<&str>| -> f64 {
            match v {
                Val::Arr(_) => pa_core::parse_gps_coordinate(pa_core::GpsValue::Parts(p), r),
                _ => pa_core::parse_gps_coordinate(gps_value(v), r),
            }
            .unwrap_or(f64::NAN)
        };
        lat = one(la, &pla, ref_of("GPSLatitudeRef"));
        lon = one(lo, &plo, ref_of("GPSLongitudeRef"));
    }
    if pa_core::is_valid_gps(lat, lon) {
        m.lat = Some(js::round(lat * 1e6) / 1e6);
        m.lon = Some(js::round(lon * 1e6) / 1e6);
    }
    if let (Some(local), None) = (m.local, m.offset) {
        if let Some(date) = get(ex, "GPSDateStamp").as_str() {
            let time = get(ex, "GPSTimeStamp");
            let parts: Vec<f64>;
            let t = match time {
                Val::Arr(a) => {
                    parts = a.iter().map(Val::number).collect();
                    Some(pa_core::GpsTime::Parts(&parts))
                }
                Val::Str(s) => Some(pa_core::GpsTime::Text(s)),
                _ => None,
            };
            if let Some(gps) = t.and_then(|t| pa_core::parse_gps_date_time(date, t)) {
                m.offset = pa_core::offset_from_gps(local, gps);
            }
        }
    }
    let s = |v: &Val| -> Option<String> {
        let x = v.as_str()?.replace('\0', "");
        let t = js::trim(&x);
        if t.is_empty() {
            None
        } else {
            Some(js::slice16_to(t, 80).to_string())
        }
    };
    m.make = s(get(ex, "Make"));
    m.model = s(get(ex, "Model"));
    m.lens = s(get(ex, "LensModel")).or_else(|| s(get(ex, "Lens")));
    let proj = match get(ex, "ProjectionType") {
        Val::Undef => String::new(),
        v => v.js_string().to_lowercase(),
    };
    let use_pano = get(ex, "UsePanoramaViewer");
    m.gpano = proj == "equirectangular"
        || proj == "cylindrical"
        || *use_pano == Val::Bool(true)
        || use_pano.js_string().to_lowercase() == "true";
}

/// Image metadata from its bytes (`imageMeta`): sharp-like size probe + exifr tags.
pub fn image_meta(src: &dyn Source, ext: &str, skip_pixels: bool) -> ExtractedMeta {
    let mut m = ExtractedMeta {
        playback: if pa_core::BROWSER_IMAGE_EXT.contains(&ext) {
            Playback::Direct
        } else {
            Playback::Convert
        },
        ..Default::default()
    };
    if ext != "heic" && ext != "heif" && !skip_pixels {
        if let Some(d) = crate::dims::probe(src) {
            m.width = Some(d.width as f64);
            m.height = Some(d.height as f64);
            m.pages = d.pages;
        }
    }
    if !matches!(ext, "svg" | "gif" | "bmp" | "ico") {
        if let Some(ex) = exifr::parse(src, &Mode::Group) {
            apply_exif(&mut m, &ex);
        }
    }
    m
}

/// container tags that carry an ISO 6709 location, most specific first (keys are lower-cased)
const VIDEO_LOCATION_TAGS: [&str; 10] = [
    "com.apple.quicktime.location.iso6709",
    "location",
    "location-eng",
    "©xyz",
    "xyz",
    "com.android.location",
    "com.samsung.android.location",
    "com.samsung.location",
    "gps",
    "gpscoordinates",
];

/// "+41.0123+028.9744/" (ISO 6709) or "41.0123, 28.9744" / "41.0123 28.9744" (decimal pair)
pub fn parse_video_location(v: &str) -> Option<pa_core::LatLon> {
    if let Some(l) = pa_core::parse_iso6709(v) {
        return Some(l);
    }
    if js::len16(v) > 80 {
        return None;
    }
    // /^\s*([+-]?\d{1,2}(?:\.\d+)?)\s*[,; ]\s*([+-]?\d{1,3}(?:\.\d+)?)(?:\s*[,; ]\s*[+-]?\d+(?:\.\d+)?)?\s*$/
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        let sp = r"[\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]";
        regex::Regex::new(&format!(
            r"^{sp}*([+-]?[0-9]{{1,2}}(?:\.[0-9]+)?){sp}*[,; ]{sp}*([+-]?[0-9]{{1,3}}(?:\.[0-9]+)?)(?:{sp}*[,; ]{sp}*[+-]?[0-9]+(?:\.[0-9]+)?)?{sp}*$"
        ))
        .expect("regex")
    });
    let c = RE.captures(v)?;
    let lat = js::number(c.get(1)?.as_str());
    let lon = js::number(c.get(2)?.as_str());
    if pa_core::is_valid_gps(lat, lon) {
        Some(pa_core::LatLon { lat, lon })
    } else {
        None
    }
}

/// Grouping fields read from ffprobe tags (`videoGroupMeta`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VideoGroupMeta {
    /// QuickTime local time (only with an offset)
    pub local: Option<i64>,
    /// its offset
    pub offset: Option<i32>,
    /// creation_time
    pub instant: Option<i64>,
    /// latitude
    #[serde(with = "js::num_opt")]
    pub lat: Option<f64>,
    /// longitude
    #[serde(with = "js::num_opt")]
    pub lon: Option<f64>,
    /// make
    pub make: Option<String>,
    /// model
    pub model: Option<String>,
}

/// GPS / camera / capture time from ffprobe tags (QuickTime, Android, Samsung, generic).
pub fn video_group_meta(p: Option<&ProbeResult>) -> VideoGroupMeta {
    let empty = Default::default();
    let t = p.map(|p| &p.tags).unwrap_or(&empty);
    let mut loc = None;
    for k in VIDEO_LOCATION_TAGS {
        loc = t.get(k).and_then(|v| parse_video_location(v));
        if loc.is_some() {
            break;
        }
    }
    if loc.is_none() {
        for (k, v) in t {
            if !(k.contains("location") || k.contains("xyz") || k.contains("gps")) {
                continue;
            }
            loc = parse_video_location(v);
            if loc.is_some() {
                break;
            }
        }
    }
    let apple = t
        .get("com.apple.quicktime.creationdate")
        .and_then(|s| pa_core::parse_date_time_with_offset(s));
    let pick = |keys: [&str; 3]| keys.iter().find_map(|k| t.get(*k)).cloned();
    VideoGroupMeta {
        local: apple.filter(|a| a.offset.is_some()).map(|a| a.local),
        offset: apple.and_then(|a| a.offset),
        instant: p.and_then(|p| p.creation_time),
        lat: loc.map(|l| l.lat),
        lon: loc.map(|l| l.lon),
        make: pick([
            "com.apple.quicktime.make",
            "com.android.manufacturer",
            "make",
        ]),
        model: pick(["com.apple.quicktime.model", "com.android.model", "model"]),
    }
}

/// Video / audio metadata from a probe result (`extractEmbedded` for non-images).
pub fn av_meta(kind: MediaKind, ext: &str, p: Option<&ProbeResult>) -> ExtractedMeta {
    if kind == MediaKind::Audio {
        return ExtractedMeta {
            duration: p.and_then(|p| p.duration),
            ..Default::default()
        };
    }
    if kind != MediaKind::Video {
        return ExtractedMeta::default();
    }
    let g = video_group_meta(p);
    ExtractedMeta {
        local: g.local,
        offset: g.offset,
        instant: g.instant,
        lat: g.lat,
        lon: g.lon,
        make: g.make,
        model: g.model,
        width: p.and_then(|p| p.width).map(|w| w as f64),
        height: p.and_then(|p| p.height).map(|h| h as f64),
        duration: p.and_then(|p| p.duration),
        playback: if pa_core::is_browser_playable(ext, p.and_then(|p| p.codec.as_deref())) {
            Playback::Direct
        } else {
            Playback::Transcode
        },
        extra: p.map(|p| VideoExtra {
            codec: p.codec.clone(),
            audio_codec: p.audio_codec.clone(),
            fps: p.fps,
            bitrate: p.bitrate,
            rotation: p.rotation,
            has_audio: p.has_audio,
        }),
        ..Default::default()
    }
}

// ───────────────────────── Google Takeout sidecars ─────────────────────────

const JSON_DIR_TTL: Duration = Duration::from_secs(20);

type DirCache = HashMap<std::path::PathBuf, (Instant, Vec<String>)>;

static JSON_DIRS: LazyLock<Mutex<DirCache>> = LazyLock::new(Default::default);

fn json_names_in(dir: &Path) -> Vec<String> {
    let now = Instant::now();
    if let Ok(g) = JSON_DIRS.lock() {
        if let Some((at, names)) = g.get(dir) {
            if now.duration_since(*at) < JSON_DIR_TTL {
                return names.clone();
            }
        }
    }
    let names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| {
                    js::len16(n) > 5
                        && n.len() >= 5
                        && n.as_bytes()[n.len() - 5..].eq_ignore_ascii_case(b".json")
                })
                .collect()
        })
        .unwrap_or_default();
    if let Ok(mut g) = JSON_DIRS.lock() {
        if g.len() > 256 {
            g.clear();
        }
        g.insert(dir.to_path_buf(), (now, names.clone()));
    }
    names
}

/// forget cached directory listings (tests, rescans)
pub fn clear_sidecar_cache() {
    if let Ok(mut g) = JSON_DIRS.lock() {
        g.clear();
    }
}

/// Fill time / location from a parsed Takeout JSON — only what the file itself lacks. true = used.
pub fn apply_takeout(m: &mut ExtractedMeta, j: &serde_json::Value) -> bool {
    let Some(t) = pa_core::parse_takeout_json(j) else {
        return false;
    };
    let mut used = false;
    if let (None, Some(lat), Some(lon)) = (m.lat, t.lat, t.lon) {
        if pa_core::is_valid_gps(lat, lon) {
            m.lat = Some(lat);
            m.lon = Some(lon);
            used = true;
        }
    }
    if let (None, None, Some(ts)) = (m.local, m.instant, t.taken_at) {
        m.instant = Some(ts);
        used = true;
    }
    used
}

/// Takeout sidecar next to `abs` (`applySidecar`): picks the JSON by Google's naming rules.
pub fn apply_sidecar(m: &mut ExtractedMeta, abs: &Path) -> bool {
    let s = abs.to_string_lossy();
    let url_like = s
        .find("://")
        .is_some_and(|i| i > 0 && s[..i].bytes().all(|c| c.is_ascii_alphabetic()));
    if !abs.is_absolute() || url_like {
        return false;
    }
    let (Some(dir), Some(name)) = (abs.parent(), abs.file_name().and_then(|n| n.to_str())) else {
        return false;
    };
    let names = json_names_in(dir);
    if names.is_empty() {
        return false;
    }
    let Some(pick) = pa_core::match_sidecar(name, names.iter().map(String::as_str)) else {
        return false;
    };
    let Ok(raw) = std::fs::read(dir.join(pick)) else {
        return false;
    };
    let text = String::from_utf8_lossy(&raw);
    if js::len16(&text) > 1_000_000 {
        return false;
    }
    let Ok(j) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    apply_takeout(m, &j)
}

/// Known size of an already indexed row (enrichment): JPEGs then skip the pixel header.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Known {
    /// px
    pub width: Option<f64>,
    /// px
    pub height: Option<f64>,
}

/// `extractMeta`: everything for one file. `probe`: ffprobe result for videos / audio (pa-media).
/// Images are read from disk (headers / metadata segments only); Takeout sidecars are applied when the
/// file lacks a location or a capture time.
pub fn extract_meta(
    abs: &Path,
    kind: MediaKind,
    ext: &str,
    probe: Option<&ProbeResult>,
    known: Option<Known>,
) -> ExtractedMeta {
    let skip = known
        .is_some_and(|k| k.width.is_some_and(|w| w != 0.0) && k.height.is_some_and(|h| h != 0.0))
        && SINGLE_FRAME.contains(&ext);
    let mut m = match kind {
        MediaKind::Image => match std::fs::File::open(abs).and_then(ReaderSource::new) {
            Ok(src) => image_meta(&src, ext, skip),
            Err(_) => image_meta(&Vec::<u8>::new(), ext, true),
        },
        _ => av_meta(kind, ext, probe),
    };
    if matches!(kind, MediaKind::Image | MediaKind::Video)
        && (m.lat.is_none() || (m.local.is_none() && m.instant.is_none()))
    {
        apply_sidecar(&mut m, abs);
    }
    m
}

/// Row fields [`group_columns`] needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowInfo {
    /// file name
    pub name: String,
    /// folder-relative dir
    pub dir: String,
    /// extension
    pub ext: String,
    /// kind ('image', 'video', …)
    pub kind: String,
    /// ms epoch
    pub mtime: i64,
}

/// Turn extracted metadata into DB columns (time resolution, reverse geocoding, camera, flags).
pub fn group_columns(
    row: &RowInfo,
    m: &ExtractedMeta,
    geo: Option<&GeoIndex>,
    home_time_zone: &str,
) -> GroupColumns {
    let (time, src) = if let Some(local) = m.local {
        (
            pa_core::resolve_from_local(local, m.offset, home_time_zone),
            if m.offset.is_some() {
                time_src::LOCAL_OFFSET
            } else {
                time_src::LOCAL
            },
        )
    } else if let Some(i) = m.instant {
        (
            pa_core::resolve_from_instant(i, home_time_zone),
            time_src::INSTANT,
        )
    } else {
        (
            pa_core::resolve_from_instant(row.mtime, home_time_zone),
            time_src::MTIME,
        )
    };
    let cam = pa_core::normalize_camera(m.make.as_deref(), m.model.as_deref());
    let valid = pa_core::is_valid_gps_opt(m.lat, m.lon);
    let place = if valid {
        geo.and_then(|g| g.nearest(m.lat.unwrap_or(0.0), m.lon.unwrap_or(0.0), None))
    } else {
        None
    };
    let flags = pa_core::classify(&pa_core::ClassifyInput {
        name: row.name.clone(),
        dir: Some(row.dir.clone()),
        ext: row.ext.clone(),
        kind: row.kind.clone(),
        width: m.width,
        height: m.height,
        make: m.make.clone(),
        model: m.model.clone(),
        lens: m.lens.clone(),
        gpano: Some(m.gpano),
        pages: m.pages.map(f64::from),
    });
    GroupColumns {
        taken_at: time.taken_at,
        taken_local: time.local_at,
        tz_off: time.tz_offset,
        time_src: src,
        lat: if valid { m.lat } else { None },
        lon: if valid { m.lon } else { None },
        geo_id: place.as_ref().map(|p| p.id as i64),
        cc: place.as_ref().map(|p| p.cc.clone()),
        admin1: place.as_ref().and_then(|p| p.admin1_code.clone()),
        city: place
            .as_ref()
            .map(|p| p.city.clone())
            .filter(|c| !c.is_empty()),
        make: cam.make,
        model: cam.model,
        lens: m.lens.clone(),
        flags,
    }
}

/// Full EXIF for the info panel (`readExif`, on demand). `local_tz`: zone of the server process
/// (EXIF dates are wall clock; the TS server read them as local time of the process).
pub fn read_exif(
    abs: &Path,
    local_tz: &str,
) -> Option<indexmap::IndexMap<String, pa_core::ExifValue>> {
    let src = ReaderSource::new(std::fs::File::open(abs).ok()?).ok()?;
    exifr::read_exif(&src, local_tz)
}
