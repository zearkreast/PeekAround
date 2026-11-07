//! Device-local UI preferences (the web UI's `store/prefs.ts` subset the native UI uses so far),
//! persisted as JSON next to the app's other per-user data (`ui-prefs.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Accent swatches of Settings → Appearance (`ACCENTS`).
pub const ACCENTS: [&str; 8] = [
    "#8b5cf6", "#ec4899", "#f97316", "#10b981", "#06b6d4", "#3b82f6", "#eab308", "#ef4444",
];

/// Preferences. Field names / values match the web UI's `Prefs` (camelCase JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// "tr" | "en"
    pub lang: String,
    /// "dark" | "light" | "system"
    pub theme: String,
    /// accent colour (#rrggbb)
    pub accent: String,
    /// "full" | "reduced" | "off"
    pub motion: String,
    /// wide sidebar instead of the rail (not yet in the native UI)
    pub wide_sidebar: bool,
    /// "locale" | "iso"
    pub date_format: String,
    /// "auto" | "h23" | "h12"
    pub hour_cycle: String,
    /// "mon" | "sun" | "sat"
    pub week_start: String,
    /// default sort of folder views
    pub sort: String,
    /// "desc" | "asc"
    pub order: String,
    /// "justified" | "square"
    pub grid_mode: String,
    /// target row height (desktop)
    pub row_height: f64,
    /// density level 0..3 (desktop)
    pub grid_zoom_desktop: i32,
    // ── viewer & video (Settings → Viewer / Video; used by src/viewer.rs, src/video.rs) ──
    /// info panel open (remembered)
    pub info_open: bool,
    /// "remember" | "open" | "closed": the info panel when the viewer opens
    pub info_default: String,
    /// "blur" | "black" | "dark" | "tint"
    pub viewer_bg: String,
    /// filmstrip under the picture (desktop)
    pub filmstrip: bool,
    /// +/- zoom factor
    pub zoom_step: f64,
    /// double-click zoom level
    pub dbl_zoom: f64,
    /// start videos when opened
    pub video_autoplay: bool,
    /// start videos muted
    pub video_muted: bool,
    /// loop videos shorter than this many seconds (0 = never)
    pub loop_short_sec: f64,
    /// default playback speed
    pub video_speed: f64,
    /// remember the volume
    pub remember_volume: bool,
    /// last volume (0..1)
    pub video_volume: f64,
    /// privacy: location hidden until revealed
    pub hide_location: bool,
    /// ask before deleting
    pub confirm_delete: bool,
    // ── view filter, gallery tiles, library pages, memories, slideshow defaults ──
    /// view filter: "all" | "image" | "video" | "favorites"
    pub kind: String,
    /// "auto" | "day" | "month"
    pub group_by: String,
    /// date headers in the grid
    pub date_headers: bool,
    /// file names under tiles
    pub tile_names: bool,
    /// video duration badges
    pub tile_duration: bool,
    /// heart on favorite tiles
    pub tile_favorites: bool,
    /// play video previews on hover
    pub hover_preview: bool,
    /// memories strip on the timeline
    pub show_memories: bool,
    /// slideshow builder settings (1.x kept them as `peek.slideshow`)
    pub slideshow: crate::slideshow::SlideSettings,
    /// sub folder cards: "name" | "date" | "count"
    pub folder_sort: String,
    /// All | Folders per library / folder ("lib:<id>" / "dir:<id>" → "all" | "folders")
    pub library_view: std::collections::BTreeMap<String, String>,
    /// slideshow: seconds per slide
    pub slide_duration: f64,
    /// slideshow: transition
    pub slide_transition: String,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            lang: system_lang().into(),
            theme: "dark".into(),
            accent: ACCENTS[0].into(),
            motion: "full".into(),
            wide_sidebar: false,
            date_format: "locale".into(),
            hour_cycle: "auto".into(),
            week_start: "mon".into(),
            sort: "takenAt".into(),
            order: "desc".into(),
            grid_mode: "justified".into(),
            row_height: 220.0,
            grid_zoom_desktop: 1,
            info_open: false,
            info_default: "remember".into(),
            viewer_bg: "blur".into(),
            filmstrip: true,
            zoom_step: 1.4,
            dbl_zoom: 2.5,
            video_autoplay: true,
            video_muted: false,
            loop_short_sec: 0.0,
            video_speed: 1.0,
            remember_volume: true,
            video_volume: 1.0,
            hide_location: false,
            confirm_delete: true,
            kind: "all".into(),
            group_by: "auto".into(),
            date_headers: true,
            tile_names: false,
            tile_duration: true,
            tile_favorites: true,
            hover_preview: true,
            show_memories: true,
            slideshow: Default::default(),
            folder_sort: "name".into(),
            library_view: Default::default(),
            slide_duration: 4.0,
            slide_transition: "fade".into(),
        }
    }
}

/// English when the OS language is English, Turkish otherwise (the web UI's rule).
fn system_lang() -> &'static str {
    let env = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()));
    match env {
        Some(v) if v.to_ascii_lowercase().starts_with("en") => "en",
        _ => "tr",
    }
}

impl Prefs {
    /// Clamp / normalize values read from disk.
    pub fn sanitized(mut self) -> Prefs {
        let d = Prefs::default();
        let pick = |v: &mut String, allowed: &[&str], def: &str| {
            if !allowed.contains(&v.as_str()) {
                *v = def.to_string();
            }
        };
        pick(&mut self.lang, &["tr", "en"], &d.lang);
        pick(&mut self.theme, &["dark", "light", "system"], "dark");
        pick(&mut self.motion, &["full", "reduced", "off"], "full");
        pick(&mut self.date_format, &["locale", "iso"], "locale");
        pick(&mut self.hour_cycle, &["auto", "h23", "h12"], "auto");
        pick(&mut self.week_start, &["mon", "sun", "sat"], "mon");
        pick(
            &mut self.sort,
            &["takenAt", "name", "mtime", "size", "kind"],
            "takenAt",
        );
        pick(&mut self.order, &["desc", "asc"], "desc");
        pick(&mut self.grid_mode, &["justified", "square"], "justified");
        if crate::theme::parse_hex(&self.accent).is_none() {
            self.accent = ACCENTS[0].into();
        }
        self.row_height = if self.row_height.is_finite() {
            self.row_height.clamp(110.0, 420.0)
        } else {
            220.0
        };
        self.grid_zoom_desktop = self.grid_zoom_desktop.clamp(0, 3);
        pick(&mut self.info_default, &["remember", "open", "closed"], "remember");
        pick(&mut self.viewer_bg, &["blur", "black", "dark", "tint"], "blur");
        let num = |v: f64, lo: f64, hi: f64, def: f64| if v.is_finite() { v.clamp(lo, hi) } else { def };
        self.zoom_step = num(self.zoom_step, 1.1, 3.0, 1.4);
        self.dbl_zoom = num(self.dbl_zoom, 1.5, 8.0, 2.5);
        self.loop_short_sec = num(self.loop_short_sec, 0.0, 600.0, 0.0);
        self.video_speed = num(self.video_speed, 0.25, 2.0, 1.0);
        self.video_volume = num(self.video_volume, 0.0, 1.0, 1.0);
        pick(&mut self.kind, &["all", "image", "video", "favorites"], "all");
        pick(&mut self.group_by, &["auto", "day", "month"], "auto");
        pick(&mut self.folder_sort, &["name", "date", "count"], "name");
        self.library_view.retain(|_, v| v == "all" || v == "folders");
        self.slide_duration = num(self.slide_duration, 0.5, 60.0, 4.0);
        pick(
            &mut self.slide_transition,
            &["none", "fade", "slide", "zoom", "kenburns", "blur", "flip", "circle", "random"],
            "fade",
        );
        self
    }

    /// Set any field by its JSON (camelCase) name; `value` is parsed as JSON when it is a
    /// number / bool, else taken as a string. Returns false for unknown keys / wrong types.
    pub fn set_by_key(&mut self, key: &str, value: &str) -> bool {
        let Ok(serde_json::Value::Object(mut obj)) = serde_json::to_value(&*self) else {
            return false;
        };
        let Some(old) = obj.get(key) else {
            return false;
        };
        let v = match old {
            serde_json::Value::String(_) => serde_json::Value::String(value.to_string()),
            serde_json::Value::Bool(_) => serde_json::Value::Bool(value == "1" || value == "true"),
            _ => match serde_json::from_str::<serde_json::Value>(value) {
                Ok(v) => v,
                Err(_) => return false,
            },
        };
        obj.insert(key.to_string(), v);
        match serde_json::from_value::<Prefs>(serde_json::Value::Object(obj)) {
            Ok(p) => {
                *self = p.sanitized();
                true
            }
            Err(_) => false,
        }
    }

    /// Load (defaults when missing / unreadable).
    pub fn load(path: &Path) -> Prefs {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Prefs>(&b).ok())
            .unwrap_or_default()
            .sanitized()
    }

    /// Save atomically (write + rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp: PathBuf = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_sanitize() {
        let dir = std::env::temp_dir().join(format!("pa-ui-prefs-{}", std::process::id()));
        let p = dir.join("ui-prefs.json");
        let mut prefs = Prefs {
            lang: "en".into(),
            theme: "light".into(),
            accent: "#10b981".into(),
            ..Prefs::default()
        };
        prefs.row_height = 300.0;
        prefs.save(&p).unwrap();
        assert_eq!(Prefs::load(&p), prefs);
        std::fs::write(
            &p,
            r##"{"theme":"neon","accent":"red","rowHeight":9999,"motion":"off","unknown":1}"##,
        )
        .unwrap();
        let l = Prefs::load(&p);
        assert_eq!(
            (
                l.theme.as_str(),
                l.accent.as_str(),
                l.row_height,
                l.motion.as_str()
            ),
            ("dark", "#8b5cf6", 420.0, "off")
        );
        assert_eq!(Prefs::load(&dir.join("missing.json")).theme, "dark");
        let mut p = Prefs::default();
        assert!(p.set_by_key("dblZoom", "4"));
        assert!(p.set_by_key("filmstrip", "false"));
        assert!(p.set_by_key("viewerBg", "black"));
        assert!(!p.set_by_key("nope", "1"));
        assert!(!p.set_by_key("zoomStep", "abc"));
        assert_eq!((p.dbl_zoom, p.filmstrip, p.viewer_bg.as_str()), (4.0, false, "black"));
        assert!(p.set_by_key("videoVolume", "7"));
        assert_eq!(p.video_volume, 1.0);
        std::fs::remove_dir_all(dir).ok();
    }
}
