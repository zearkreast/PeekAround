//! Offline reverse geocoder backed by the GeoNames asset of the web UI (mirrors `geo.ts`).
//! Looked up in `$PEEK_GEO_FILE`, `<webDir>/geo/cities.bin`, then the repository's
//! `assets/geo/cities.bin` (development / tests).

use pa_core::{GeoIndex, GeoMatch, PlaceInfo};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The geocoder (index loaded lazily on first use).
pub struct GeoService {
    pub file: Option<PathBuf>,
    index: OnceLock<Option<GeoIndex>>,
    place_text: Mutex<HashMap<String, String>>,
}

/// `${cc}-${admin1 || '00'}`
pub fn place_key(cc: &str, admin1: Option<&str>) -> String {
    match admin1 {
        Some(a) if !a.is_empty() => format!("{cc}-{a}"),
        _ => format!("{cc}-00"),
    }
}

impl GeoService {
    /// locate the asset (`web_dir` = the served web UI)
    pub fn new(web_dir: Option<&Path>) -> Self {
        Self {
            file: Self::locate(web_dir),
            index: OnceLock::new(),
            place_text: Mutex::new(HashMap::new()),
        }
    }

    /// candidate files, first existing one wins
    pub fn locate(web_dir: Option<&Path>) -> Option<PathBuf> {
        let mut c: Vec<PathBuf> = Vec::new();
        if let Ok(f) = std::env::var("PEEK_GEO_FILE") {
            if !f.is_empty() {
                c.push(f.into());
            }
        }
        if let Some(w) = web_dir {
            c.push(w.join(pa_core::GEO_ASSET_PATH));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                c.push(d.join("web").join(pa_core::GEO_ASSET_PATH));
                c.push(d.join("../web").join(pa_core::GEO_ASSET_PATH));
            }
        }
        c.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets")
                .join(pa_core::GEO_ASSET_PATH),
        );
        c.into_iter().find(|f| f.is_file())
    }

    pub fn index(&self) -> Option<&GeoIndex> {
        self.index
            .get_or_init(|| {
                let bytes = std::fs::read(self.file.as_ref()?).ok()?;
                pa_core::decode_geo_asset(&bytes).ok()
            })
            .as_ref()
    }

    /// asset present and readable
    pub fn available(&self) -> bool {
        self.index().is_some()
    }

    pub fn nearest(&self, lat: Option<f64>, lon: Option<f64>) -> Option<GeoMatch> {
        self.index()?.nearest(lat?, lon?, None)
    }

    /// PlaceInfo from stored columns (city label + cc + admin1 code)
    pub fn place_info(
        &self,
        city: Option<&str>,
        cc: Option<&str>,
        admin1: Option<&str>,
    ) -> Option<PlaceInfo> {
        let cc = cc.filter(|c| !c.is_empty())?;
        Some(PlaceInfo {
            city: city.unwrap_or("").to_string(),
            admin1: self.admin1_name(cc, admin1),
            cc: cc.to_string(),
            key: place_key(cc, admin1),
        })
    }

    pub fn admin1_name(&self, cc: &str, code: Option<&str>) -> Option<String> {
        self.index()?.admin1_name(cc, Some(code.unwrap_or("")))
    }

    /// country display names (en, tr, de, fr, es) for search
    pub fn country_names_of(cc: &str) -> &'static [&'static str] {
        match crate::country_names::COUNTRY_NAMES.binary_search_by(|(c, _)| (*c).cmp(cc)) {
            Ok(i) => crate::country_names::COUNTRY_NAMES[i].1,
            Err(_) => &[],
        }
    }

    /// folded "admin1 country names" text for search matching (cached per place group)
    pub fn place_text(&self, cc: Option<&str>, admin1: Option<&str>) -> String {
        let Some(cc) = cc.filter(|c| !c.is_empty()) else {
            return String::new();
        };
        let k = format!("{cc}-{}", admin1.unwrap_or(""));
        if let Some(t) = self.place_text.lock().get(&k) {
            return t.clone();
        }
        let mut parts = vec![self.admin1_name(cc, admin1).unwrap_or_default()];
        parts.extend(Self::country_names_of(cc).iter().map(|s| s.to_string()));
        let t = pa_core::fold_text(Some(&parts.join(" ")));
        self.place_text.lock().insert(k, t.clone());
        t
    }
}
