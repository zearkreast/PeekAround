//! What the phone answers (1.x `PeekMediaServer` / `PeekFs` / `PeekMeta`): list metadata, thumbnails,
//! display renditions, video strips, details. Images are decoded here (pa-media); videos go to
//! the [`Platform`] (Android: MediaMetadataRetriever through JNI).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::codec::{self, LineMeta, LinkError};
use crate::prefs::LinkStatus;

/// a video's metadata
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoInfo {
    /// oriented width
    pub width: u32,
    /// oriented height
    pub height: u32,
    /// seconds
    pub duration: f64,
    /// creation instant (ms)
    pub taken: Option<i64>,
    /// ISO 6709 location
    pub location: Option<String>,
}

/// What the link needs from the OS. Every method has a "not available" default.
pub trait Platform: Send + Sync + 'static {
    /// the link state changed (notification text)
    fn status(&self, _s: &LinkStatus) {}
    /// requests being served (Wi-Fi / wake locks while > 0)
    fn active(&self, _n: usize) {}
    /// a fast system thumbnail (MediaStore), longest side about `size`
    fn system_thumb(&self, _path: &Path, _kind: &str, _size: u32) -> Option<Vec<u8>> {
        None
    }
    /// JPEG frame of a video, longest side ≤ `max`
    fn video_frame(&self, _path: &Path, _max: u32) -> Option<Vec<u8>> {
        None
    }
    /// JPEG sprite of `n` frames, `h` px high
    fn video_strip(&self, _path: &Path, _n: u32, _h: u32) -> Option<Vec<u8>> {
        None
    }
    /// size / duration / time / place of a video
    fn video_info(&self, _path: &Path) -> Option<VideoInfo> {
        None
    }
    /// a file was written into a shared folder (MediaScanner)
    fn scanned(&self, _path: &Path) {}
}

/// no OS integration (tests, desktop)
pub struct NoPlatform;
impl Platform for NoPlatform {}

/// list metadata cache keyed by (path, size, mtime): re-lists after `changed` stay cheap
#[derive(Default)]
pub struct MetaCache(Mutex<HashMap<PathBuf, (u64, i64, LineMeta)>>);

impl MetaCache {
    /// metadata of one listed file
    pub fn get(&self, p: &Path, kind: &str, size: u64, mtime: i64, pf: &dyn Platform) -> LineMeta {
        if let Some((s, m, meta)) = self.0.lock().unwrap().get(p) {
            if *s == size && *m == mtime {
                return *meta;
            }
        }
        let meta = match kind {
            "image" => image_line_meta(p),
            "video" => pf
                .video_info(p)
                .map(|v| LineMeta {
                    w: (v.width > 0).then_some(v.width),
                    h: (v.height > 0).then_some(v.height),
                    d: (v.duration > 0.0).then_some(v.duration),
                    t: v.taken,
                    id: None,
                })
                .unwrap_or_default(),
            _ => LineMeta::default(),
        };
        let mut c = self.0.lock().unwrap();
        if c.len() > 200_000 {
            c.clear();
        }
        c.insert(p.to_path_buf(), (size, mtime, meta));
        meta
    }
}

/// wall clock "as UTC" (EXIF) → instant, in the phone's zone when the file has no offset
fn local_to_instant(local: i64, offset_min: Option<i32>) -> i64 {
    if let Some(o) = offset_min {
        return local - o as i64 * 60_000;
    }
    use chrono::{Local, Offset, TimeZone};
    let naive = chrono::DateTime::from_timestamp_millis(local).map(|d| d.naive_utc());
    let off = naive
        .and_then(|n| Local.offset_from_local_datetime(&n).earliest())
        .map(|o| o.fix().local_minus_utc() as i64)
        .unwrap_or(0);
    local - off * 1000
}

fn image_line_meta(p: &Path) -> LineMeta {
    let mut out = LineMeta::default();
    if let Ok(i) = pa_media::image_info_path(p) {
        let (w, h) = i.display_size();
        if w > 0 && h > 0 {
            out.w = Some(w);
            out.h = Some(h);
        }
    }
    let ext = codec::ext(&p.to_string_lossy());
    if let Ok(f) = std::fs::File::open(p) {
        if let Ok(src) = pa_meta::ReaderSource::new(f) {
            let m = pa_meta::image_meta(&src, &ext, true);
            out.t = m
                .instant
                .or_else(|| m.local.map(|l| local_to_instant(l, m.offset)));
        }
    }
    out
}

fn jpeg(img: &image::DynamicImage, q: u8) -> Result<Vec<u8>, LinkError> {
    pa_media::encode::jpeg(img, pa_media::encode::JpegOptions::new(q))
        .map_err(|e| LinkError::new(500, e.to_string()))
}

fn decode_fit(p: &Path, max: u32) -> Result<image::DynamicImage, LinkError> {
    let img = pa_media::decode_path(p, pa_media::DecodeOptions::default().fit(max, max))
        .map_err(|_| LinkError::new(500, "cannot decode"))?;
    pa_media::resize::into_inside(img, max, max, true)
        .map_err(|_| LinkError::new(500, "cannot decode"))
}

/// thumbnail buckets of the server
pub fn bucket(s: u32) -> u32 {
    if s <= 320 {
        320
    } else if s <= 640 {
        640
    } else {
        1600
    }
}

/// `thumb`: JPEG, longest side = the bucket of `size`
pub fn thumb(p: &Path, kind: &str, size: u32, pf: &dyn Platform) -> Result<Vec<u8>, LinkError> {
    let s = bucket(size);
    let ext = codec::ext(&p.to_string_lossy());
    if s <= 640 && ext != "gif" {
        if let Some(b) = pf.system_thumb(p, kind, s) {
            return Ok(b);
        }
    }
    match kind {
        "image" => jpeg(&decode_fit(p, s)?, if s >= 1600 { 88 } else { 80 }),
        "video" => pf
            .video_frame(p, s)
            .ok_or_else(|| LinkError::new(500, "cannot decode")),
        _ => Err(LinkError::new(404, "no thumbnail")),
    }
}

/// `display`: JPEG ≤ `max` px (256…8192)
pub fn display(p: &Path, kind: &str, max: u32, pf: &dyn Platform) -> Result<Vec<u8>, LinkError> {
    let max = max.clamp(256, 8192);
    match kind {
        "video" => pf
            .video_frame(p, max)
            .ok_or_else(|| LinkError::new(500, "cannot decode")),
        _ => jpeg(&decode_fit(p, max)?, 88),
    }
}

/// `strip`: `n` (2…60) frames, `h` (32…240) px high
pub fn strip(p: &Path, kind: &str, n: u32, h: u32, pf: &dyn Platform) -> Result<Vec<u8>, LinkError> {
    if kind != "video" {
        return Err(LinkError::new(400, "not a video"));
    }
    pf.video_strip(p, n.clamp(2, 60), h.clamp(32, 240))
        .ok_or_else(|| LinkError::new(500, "cannot decode"))
}

fn val_json(v: &pa_meta::Val) -> Option<Value> {
    use pa_meta::Val;
    match v {
        Val::Num(n) if n.is_finite() => serde_json::Number::from_f64(*n).map(Value::Number),
        Val::Str(s) => {
            let s = s.trim_matches(char::from(0)).trim();
            (!s.is_empty()).then(|| Value::String(s.to_string()))
        }
        Val::Bool(b) => Some(Value::Bool(*b)),
        Val::Arr(a) if a.len() == 1 => val_json(&a[0]),
        Val::Typed(a) if a.len() == 1 => serde_json::Number::from_f64(a[0]).map(Value::Number),
        _ => None,
    }
}

// the EXIF fields 1.x PeekMeta.details sent (ExifInterface names → exiftool names)
const DETAIL_TAGS: [&str; 24] = [
    "Make",
    "Model",
    "LensModel",
    "DateTimeOriginal",
    "CreateDate",
    "ModifyDate",
    "OffsetTimeOriginal",
    "OffsetTimeDigitized",
    "OffsetTime",
    "GPSDateStamp",
    "GPSTimeStamp",
    "FNumber",
    "ExposureTime",
    "FocalLength",
    "ISO",
    "FocalLengthIn35mmFormat",
    "ExifImageWidth",
    "ExifImageHeight",
    "ImageWidth",
    "ImageHeight",
    "Orientation",
    "Software",
    "ProjectionType",
    "UsePanoramaViewer",
];

fn valid_gps(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
        && !(lat == 0.0 && lon == 0.0)
}

/// `details`: `{"exif":{…},"gps":{"lat","lon"}|null,"make","model","lens"}`
pub fn details(p: &Path, kind: &str, pf: &dyn Platform) -> Value {
    let mut exif = Map::new();
    let mut gps: Option<(f64, f64)> = None;
    let (mut make, mut model, mut lens) = (None, None, None);
    if kind == "image" {
        if let Some(tags) = std::fs::File::open(p)
            .ok()
            .and_then(|f| pa_meta::ReaderSource::new(f).ok())
            .and_then(|src| pa_meta::exifr::parse(&src, &pa_meta::exifr::Mode::Group))
        {
            for k in DETAIL_TAGS {
                if let Some(v) = tags.get(k).and_then(val_json) {
                    exif.insert(k.to_string(), v);
                }
            }
            let mut m = pa_meta::ExtractedMeta::default();
            pa_meta::apply_exif(&mut m, &tags);
            if let (Some(lat), Some(lon)) = (m.lat, m.lon) {
                if valid_gps(lat, lon) {
                    gps = Some((lat, lon));
                }
            }
            make = m.make;
            model = m.model;
            lens = m.lens;
        }
    } else if kind == "video" {
        gps = pf
            .video_info(p)
            .and_then(|v| v.location)
            .and_then(|l| pa_meta::parse_video_location(&l))
            .map(|l| (l.lat, l.lon))
            .filter(|(a, b)| valid_gps(*a, *b));
    }
    json!({
        "exif": exif,
        "gps": gps.map(|(lat, lon)| json!({"lat": lat, "lon": lon})),
        "make": make,
        "model": model,
        "lens": lens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instants_from_exif_clock() {
        assert_eq!(local_to_instant(10 * 3_600_000, Some(180)), 7 * 3_600_000);
        assert_eq!(bucket(100), 320);
        assert_eq!(bucket(641), 1600);
    }

    #[test]
    fn image_ops_on_a_real_jpeg() {
        let d = std::env::temp_dir().join(format!("pa-phone-ops-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("a.jpg");
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(800, 400, |x, _| {
            image::Rgb([(x % 255) as u8, 80, 160])
        }));
        std::fs::write(&p, jpeg(&img, 85).unwrap()).unwrap();
        let t = thumb(&p, "image", 320, &NoPlatform).unwrap();
        let ti = image::load_from_memory(&t).unwrap();
        assert_eq!((ti.width(), ti.height()), (320, 160));
        let m = MetaCache::default().get(&p, "image", 1, 2, &NoPlatform);
        assert_eq!((m.w, m.h), (Some(800), Some(400)));
        let det = details(&p, "image", &NoPlatform);
        assert!(det["exif"].is_object() && det["gps"].is_null());
        assert_eq!(
            strip(&p, "image", 10, 90, &NoPlatform).unwrap_err().status,
            400
        );
        assert_eq!(
            thumb(&p, "video", 320, &NoPlatform).unwrap_err().status,
            500
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
