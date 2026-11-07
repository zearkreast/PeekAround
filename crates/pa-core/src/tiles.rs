//! Map tile styles (mirrors `tiles.ts`).

use serde::{Deserialize, Serialize};

use crate::js;

/// A map tile style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapTileStyle {
    /// OpenStreetMap
    Osm,
    /// OpenTopoMap
    Topo,
    /// Esri World Imagery
    Satellite,
}

impl MapTileStyle {
    /// id string
    pub fn as_str(self) -> &'static str {
        match self {
            MapTileStyle::Osm => "osm",
            MapTileStyle::Topo => "topo",
            MapTileStyle::Satellite => "satellite",
        }
    }
}

/// `MAP_TILE_STYLES`
pub const MAP_TILE_STYLES: [MapTileStyle; 3] = [
    MapTileStyle::Osm,
    MapTileStyle::Topo,
    MapTileStyle::Satellite,
];

/// Tile content type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TileType {
    /// PNG
    #[serde(rename = "image/png")]
    Png,
    /// JPEG
    #[serde(rename = "image/jpeg")]
    Jpeg,
}

/// One tile style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileStyleInfo {
    /// style id
    pub id: MapTileStyle,
    /// upstream URL template: {s} subdomain, {z}/{x}/{y}
    pub url: &'static str,
    /// subdomains
    pub subdomains: &'static [&'static str],
    /// max zoom
    pub max_zoom: u32,
    /// HTML attribution
    pub attribution: &'static str,
    /// plain text attribution
    pub attribution_text: &'static str,
    /// darken in dark themes
    pub dark_filter: bool,
    /// content type
    #[serde(rename = "type")]
    pub kind: TileType,
}

static OSM: TileStyleInfo = TileStyleInfo {
    id: MapTileStyle::Osm,
    url: "https://tile.openstreetmap.org/{z}/{x}/{y}.png",
    subdomains: &[],
    max_zoom: 19,
    attribution: r#"© <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noreferrer">OpenStreetMap</a> contributors"#,
    attribution_text: "© OpenStreetMap contributors",
    dark_filter: true,
    kind: TileType::Png,
};

static TOPO: TileStyleInfo = TileStyleInfo {
    id: MapTileStyle::Topo,
    url: "https://{s}.tile.opentopomap.org/{z}/{x}/{y}.png",
    subdomains: &["a", "b", "c"],
    max_zoom: 17,
    attribution: r#"© <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noreferrer">OpenStreetMap</a> contributors, SRTM · © <a href="https://opentopomap.org" target="_blank" rel="noreferrer">OpenTopoMap</a> (CC-BY-SA)"#,
    attribution_text: "© OpenStreetMap contributors, SRTM · © OpenTopoMap (CC-BY-SA)",
    dark_filter: true,
    kind: TileType::Png,
};

static SATELLITE: TileStyleInfo = TileStyleInfo {
    id: MapTileStyle::Satellite,
    url: "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}",
    subdomains: &[],
    max_zoom: 19,
    attribution: r#"Tiles © <a href="https://www.esri.com" target="_blank" rel="noreferrer">Esri</a> — Source: Esri, Maxar, Earthstar Geographics, and the GIS User Community"#,
    attribution_text: "Tiles © Esri — Source: Esri, Maxar, Earthstar Geographics, and the GIS User Community",
    dark_filter: false,
    kind: TileType::Jpeg,
};

/// `TILE_STYLES[style]`
pub fn tile_style(style: MapTileStyle) -> &'static TileStyleInfo {
    match style {
        MapTileStyle::Osm => &OSM,
        MapTileStyle::Topo => &TOPO,
        MapTileStyle::Satellite => &SATELLITE,
    }
}

/// Settings.mapTiles → tile style (legacy 'carto' / unknown → osm), None when tiles are off
pub fn tile_style_of(setting: Option<&str>) -> Option<MapTileStyle> {
    match setting {
        Some("off") => None,
        Some("topo") => Some(MapTileStyle::Topo),
        Some("satellite") => Some(MapTileStyle::Satellite),
        _ => Some(MapTileStyle::Osm),
    }
}

/// a known style id
pub fn parse_tile_style(s: &str) -> Option<MapTileStyle> {
    match s {
        "osm" => Some(MapTileStyle::Osm),
        "topo" => Some(MapTileStyle::Topo),
        "satellite" => Some(MapTileStyle::Satellite),
        _ => None,
    }
}

fn is_integer(v: f64) -> bool {
    v.is_finite() && v.trunc() == v
}

/// integer z in \[0, maxZoom\], x/y in \[0, 2^z)
pub fn is_valid_tile(style: MapTileStyle, z: f64, x: f64, y: f64) -> bool {
    if ![z, x, y].iter().all(|v| is_integer(*v)) {
        return false;
    }
    if z < 0.0 || z > tile_style(style).max_zoom as f64 {
        return false;
    }
    let n = 2f64.powf(z);
    x >= 0.0 && x < n && y >= 0.0 && y < n
}

/// upstream URL of one tile (subdomain picked deterministically)
pub fn upstream_tile_url(style: MapTileStyle, z: i64, x: i64, y: i64) -> String {
    let s = tile_style(style);
    let sub = if s.subdomains.is_empty() {
        ""
    } else {
        s.subdomains[((x + y).rem_euclid(s.subdomains.len() as i64)) as usize]
    };
    s.url
        .replacen("{s}", sub, 1)
        .replacen("{z}", &z.to_string(), 1)
        .replacen("{x}", &x.to_string(), 1)
        .replacen("{y}", &y.to_string(), 1)
}

/// Tile containing a point plus the pixel offset inside it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TilePos {
    /// tile x
    pub x: i64,
    /// tile y
    pub y: i64,
    /// pixel x inside the tile (0..256)
    #[serde(with = "js::num")]
    pub px: f64,
    /// pixel y inside the tile (0..256)
    #[serde(with = "js::num")]
    pub py: f64,
}

/// Web Mercator: tile containing (lat, lon) at zoom z
pub fn tile_of(lat: f64, lon: f64, z: u32) -> TilePos {
    let n = 2f64.powi(z as i32);
    let la = lat.clamp(-85.05112878, 85.05112878);
    let fx = (lon + 180.0) / 360.0 * n;
    let r = la * std::f64::consts::PI / 180.0;
    let fy = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    let x = fx.floor().max(0.0).min(n - 1.0);
    let y = fy.floor().max(0.0).min(n - 1.0);
    TilePos {
        x: x as i64,
        y: y as i64,
        px: (fx - x) * 256.0,
        py: (fy - y) * 256.0,
    }
}
