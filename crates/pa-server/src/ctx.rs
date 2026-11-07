//! The shared server context (mirrors `context.ts`) + settings + runtime settings
//! (`runtime-settings.ts`).

use crate::config::{ResolvedConfig, SecurityOptions};
use crate::db::Db;
use crate::events::EventHub;
use crate::geo::GeoService;
use crate::thumbs::ThumbService;
use pa_media::Ffmpeg;
use parking_lot::Mutex;
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Everything a route / service needs.
pub struct Ctx {
    pub cfg: ResolvedConfig,
    pub security: SecurityOptions,
    pub db: Arc<Db>,
    pub events: EventHub,
    pub ffmpeg: Arc<Ffmpeg>,
    pub thumbs: Arc<ThumbService>,
    pub geo: Arc<GeoService>,
    pub scanner: crate::scanner::Scanner,
    pub watcher: crate::watcher::Watcher,
    pub explore: crate::explore::Explore,
    pub devices: crate::devices::DeviceHub,
    pub remote: crate::remote::RemoteService,
    pub jobs: pa_jobs::JobService,
    /// video edit / slideshow export runners (pa-jobs)
    pub exporter: std::sync::OnceLock<pa_jobs::Exporter>,
    pub tiles: Arc<crate::tiles::TileService>,
    pub rate: crate::security::RateLimiter,
    /// desktop LAN share state (enabled, port)
    pub lan: Mutex<(bool, u16)>,
    /// cancelled on close()
    pub shutdown: CancellationToken,
    /// background loops awaited on close()
    pub tasks: TaskTracker,
    /// browse skeleton cache (key → serialized JSON)
    pub skeletons: Mutex<std::collections::VecDeque<(String, Arc<Vec<u8>>)>>,
    /// metaStatus memo (at, ms, value)
    pub meta_memo: Mutex<Option<(std::time::Instant, f64, pa_core::MetaStatus)>>,
    /// rereadMeta re-kick loop running
    pub rekick: std::sync::atomic::AtomicBool,
    /// the zone of this process (EXIF wall clocks of the info panel)
    pub local_tz: String,
}

/// shared handle
pub type C = Arc<Ctx>;

/// `DEFAULT_SETTINGS` as a JSON object (key order of the TS object)
pub fn default_settings() -> Map<String, Value> {
    match serde_json::to_value(pa_core::Settings::default()) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

impl Ctx {
    /// `{ ...DEFAULT_SETTINGS, ...stored }`
    pub fn settings_json(&self) -> Map<String, Value> {
        let mut m = default_settings();
        if let Some(Value::Object(stored)) = self.db.get_setting("settings") {
            for (k, v) in stored {
                m.insert(k, v);
            }
        }
        m
    }

    /// the typed settings (invalid stored values fall back to the defaults)
    pub fn settings(&self) -> pa_core::Settings {
        settings_of(&self.settings_json())
    }

    /// `saveSettings(ctx, patch)`
    pub fn save_settings(self: &Arc<Self>, patch: Map<String, Value>) -> Map<String, Value> {
        let mut next = self.settings_json();
        let home_changed = patch.contains_key("homeTimeZone");
        for (k, v) in patch {
            next.insert(k, v);
        }
        let _ = self
            .db
            .set_setting("settings", &Value::Object(next.clone()));
        let s = settings_of(&next);
        self.scanner.set_show_hidden(s.show_hidden);
        if home_changed {
            crate::scanner::set_home_time_zone(self, &s.home_time_zone);
        }
        self.apply_runtime_settings(&s);
        next
    }

    /// thumbnail quality + indexing concurrency + ffmpeg threads (`applyRuntimeSettings`)
    pub fn apply_runtime_settings(&self, s: &pa_core::Settings) {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        let lim = intensity_limits(s.index_intensity, cpus, self.cfg.desktop());
        self.thumbs.set_quality(match s.thumb_quality {
            pa_core::ThumbQuality::Low => pa_media::ThumbQuality::Low,
            pa_core::ThumbQuality::Normal => pa_media::ThumbQuality::Normal,
            pa_core::ThumbQuality::High => pa_media::ThumbQuality::High,
        });
        self.thumbs.limiter.resize(lim.max, lim.low_max);
        self.thumbs
            .bg
            .configure(lim.workers, std::time::Duration::from_millis(lim.gap_ms));
        self.scanner.set_concurrency(lim.workers);
        self.ffmpeg.set_decode_threads(lim.ffmpeg_threads);
        pa_media::decode::set_heif_threads(lim.ffmpeg_threads);
    }
}

/// `intensityLimits` (pa-media's table; Settings.indexIntensity)
pub fn intensity_limits(
    i: pa_core::IndexIntensity,
    cpus: usize,
    desktop: bool,
) -> pa_media::IntensityLimits {
    let ii = match i {
        pa_core::ThumbQuality::Low => pa_media::IndexIntensity::Low,
        pa_core::ThumbQuality::Normal => pa_media::IndexIntensity::Normal,
        pa_core::ThumbQuality::High => pa_media::IndexIntensity::High,
    };
    pa_media::intensity_limits(ii, cpus, desktop)
}

/// ffmpeg input options for GPU decoding
pub fn hw_accel_args(s: &pa_core::Settings) -> Vec<String> {
    if s.hw_accel {
        vec!["-hwaccel".into(), "auto".into()]
    } else {
        vec![]
    }
}

fn settings_of(m: &Map<String, Value>) -> pa_core::Settings {
    let d = pa_core::Settings::default();
    let get = |k: &str| m.get(k).cloned().unwrap_or(Value::Null);
    fn pick<T: serde::de::DeserializeOwned>(v: Value, d: T) -> T {
        serde_json::from_value(v).unwrap_or(d)
    }
    pa_core::Settings {
        zip_action: pick(get("zipAction"), d.zip_action),
        home_time_zone: pick(get("homeTimeZone"), d.home_time_zone),
        map_tiles: pick(get("mapTiles"), d.map_tiles),
        tile_cache_mb: m
            .get("tileCacheMb")
            .and_then(Value::as_f64)
            .map(|f| f as i64)
            .unwrap_or(d.tile_cache_mb),
        edit_suffix: pick(get("editSuffix"), d.edit_suffix),
        show_hidden: pick(get("showHidden"), d.show_hidden),
        temp_session_hours: m
            .get("tempSessionHours")
            .and_then(Value::as_f64)
            .map(|f| f as i64)
            .unwrap_or(d.temp_session_hours),
        host_browse_roots: pick(get("hostBrowseRoots"), d.host_browse_roots),
        remote_cache_mb: m
            .get("remoteCacheMb")
            .and_then(Value::as_f64)
            .map(|f| f as i64)
            .unwrap_or(d.remote_cache_mb),
        thumb_quality: pick(get("thumbQuality"), d.thumb_quality),
        index_intensity: pick(get("indexIntensity"), d.index_intensity),
        hw_accel: pick(get("hwAccel"), d.hw_accel),
    }
}

/// number of a settings value as f64 (`tempSessionHours` may be fractional)
pub fn setting_f64(m: &Map<String, Value>, k: &str) -> Option<f64> {
    m.get(k).and_then(Value::as_f64)
}
