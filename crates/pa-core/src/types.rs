//! API contract (mirrors `packages/shared/src/index.ts` types + constants).
//!
//! JSON shapes are byte compatible with the TypeScript server: camelCase keys, ids as strings,
//! `T | null` fields always present (`Option<T>`), `x?: T` fields omitted when `None`,
//! `x?: T | null` fields as `Option<Option<T>>` (missing / null / value, see [`crate::js::double_option`]),
//! floats formatted like `JSON.stringify` ([`crate::js::num`]).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::albums::LibraryOptions;
use crate::color::ColorBucket;
use crate::insights::{DayTime, ExploreExtras, Orientation, VideoLength};
use crate::js;
use crate::remote_folders::RemoteFolderInfo;
use crate::tiles::MapTileStyle;

/// Application name.
pub const APP_NAME: &str = "PeekAround";
/// HTTP API version.
pub const API_VERSION: u32 = 1;

// ───────────────────────── Media ─────────────────────────

/// Kind of a media file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    /// still image
    Image,
    /// video
    Video,
    /// audio
    Audio,
    /// zip archive
    Archive,
}

impl MediaKind {
    /// the JSON / DB string
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
            MediaKind::Archive => "archive",
        }
    }
}

/// How the client should play/show the original file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Playback {
    /// browser can show/play the original file
    #[default]
    Direct,
    /// image the browser can't decode (heic, tiff…) → use display url
    Convert,
    /// video the browser can't play (mkv/hevc/avi…) → use stream url
    Transcode,
}

/// One media file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    /// media id
    pub id: String,
    /// folder id
    pub folder_id: String,
    /// directory relative to the folder root, '' for the root ('/' separated)
    pub dir: String,
    /// file name
    pub name: String,
    /// lower-case extension without dot
    pub ext: String,
    /// kind
    pub kind: MediaKind,
    /// bytes
    pub size: i64,
    /// ms epoch
    pub mtime: i64,
    /// pixels
    pub width: Option<i64>,
    /// pixels
    pub height: Option<i64>,
    /// seconds (video/audio)
    #[serde(with = "js::num_opt")]
    pub duration: Option<f64>,
    /// ms epoch (true instant)
    pub taken_at: i64,
    /// wall-clock capture time encoded as UTC (ms)
    pub local_at: i64,
    /// MEDIA_FLAG bitmask
    pub flags: u32,
    /// base64 thumbhash
    pub thumbhash: Option<String>,
    /// favourite
    pub favorite: bool,
    /// playback mode
    pub playback: Playback,
}

/// EXIF value for the info panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExifValue {
    /// number
    Num(#[serde(with = "js::num")] f64),
    /// string
    Str(String),
}

/// Video details of [`MediaDetails`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    /// video codec
    pub codec: Option<String>,
    /// frames per second
    #[serde(with = "js::num_opt")]
    pub fps: Option<f64>,
    /// bits per second
    #[serde(with = "js::num_opt")]
    pub bitrate: Option<f64>,
    /// audio codec
    pub audio_codec: Option<String>,
    /// degrees
    pub rotation: i32,
}

/// Camera of [`MediaDetails`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraInfo {
    /// normalised make
    pub make: Option<String>,
    /// normalised model
    pub model: Option<String>,
    /// lens model
    pub lens: Option<String>,
}

/// Full details of one media file (`GET /api/media/:id`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaDetails {
    /// the item
    #[serde(flatten)]
    pub item: MediaItem,
    /// absolute path (desktop) or path inside the folder (server)
    pub path: String,
    /// picked EXIF tags
    pub exif: Option<IndexMap<String, Option<ExifValue>>>,
    /// video stream info
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoInfo>,
    /// location
    pub gps: Option<crate::geo::LatLon>,
    /// reverse geocoded place
    pub place: Option<PlaceInfo>,
    /// camera
    pub camera: Option<CameraInfo>,
    /// minutes east of UTC when known
    pub tz_offset: Option<i32>,
}

/// Reverse geocoded location (offline GeoNames).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceInfo {
    /// nearest populated place, e.g. "Kadıköy"
    pub city: String,
    /// first-level admin area (province/state), e.g. "İstanbul"
    pub admin1: Option<String>,
    /// ISO 3166-1 alpha-2, e.g. "TR"
    pub cc: String,
    /// grouping key used by Places: `${cc}-${admin1 code}`
    pub key: String,
}

// ───────────────────────── Libraries & folders ─────────────────────────

/// Linked phone of a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderDevice {
    /// device id
    pub id: String,
    /// device name
    pub name: String,
    /// connected now
    pub online: bool,
}

/// A library folder (or temp session / phone folder / remote link).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRef {
    /// folder id
    pub id: String,
    /// absolute path on the host
    pub path: String,
    /// display name
    pub name: String,
    /// owning library
    pub library_id: Option<String>,
    /// temp sessions are zip peeks that disappear on expiry
    pub temp: bool,
    /// ms epoch
    pub expires_at: Option<i64>,
    /// zip name of a temp session
    pub source_name: Option<String>,
    /// indexed media
    pub media_count: i64,
    /// set when this folder lives on a linked phone
    pub device: Option<FolderDevice>,
    /// set when this folder streams from a cloud link (MEGA)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "js::double_option"
    )]
    pub remote: Option<Option<RemoteFolderInfo>>,
}

/// A library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    /// library id
    pub id: String,
    /// name
    pub name: String,
    /// css color
    pub color: String,
    /// lucide icon name
    pub icon: String,
    /// sort position
    pub sort: i64,
    /// folders
    pub folders: Vec<FolderRef>,
    /// indexed media
    pub media_count: i64,
    /// picked cover photo; null = automatic
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "js::double_option"
    )]
    pub cover_id: Option<Option<String>>,
    /// scan options
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<LibraryOptions>,
}

/// A sub directory in a browse result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubFolder {
    /// name
    pub name: String,
    /// dir relative to folder root
    pub dir: String,
    /// media inside (recursive)
    pub media_count: i64,
    /// cover ids
    pub cover_ids: Vec<String>,
    /// newest localAt inside
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "js::double_option"
    )]
    pub latest: Option<Option<i64>>,
}

/// Browse sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SortKey {
    /// capture time
    #[default]
    #[serde(rename = "takenAt")]
    TakenAt,
    /// file name
    #[serde(rename = "name")]
    Name,
    /// modification time
    #[serde(rename = "mtime")]
    Mtime,
    /// size
    #[serde(rename = "size")]
    Size,
    /// kind
    #[serde(rename = "kind")]
    Kind,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    /// ascending
    Asc,
    /// descending
    #[default]
    Desc,
}

/// Kind filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum KindFilter {
    /// everything
    #[default]
    All,
    /// images
    Image,
    /// videos
    Video,
    /// favourites
    Favorites,
    /// images + videos (no audio): the Explore pages (`'image-video'`, understood by the server)
    #[serde(rename = "image-video")]
    ImageVideo,
}

/// `gps` browse filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpsFilter {
    /// items without a location
    None,
    /// items with one
    Any,
}

/// Browse query (query string of `/api/browse`, `/api/browse/skeleton`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BrowseQuery {
    /// library
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_id: Option<String>,
    /// folder
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// include media of all sub folders
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
    /// sort key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<SortKey>,
    /// direction
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<SortDir>,
    /// kind filter
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<KindFilter>,
    /// free text
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// paging
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    /// paging
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// localAt range start
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    /// localAt range end (exclusive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
    /// Places group key or a bare country code
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// CameraGroup.key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// type flag
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<MediaFlagName>,
    /// with / without location
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gps: Option<GpsFilter>,
    /// dominant colour
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorBucket>,
    /// orientation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orient: Option<Orientation>,
    /// night / golden hour
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daytime: Option<DayTime>,
    /// short / long videos
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vlen: Option<VideoLength>,
    /// only exact duplicates
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dup: Option<bool>,
    /// city label inside `place`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// newest additions first
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_added: Option<bool>,
    /// only the items of this album
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    /// album custom order
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_position: Option<bool>,
}

/// Breadcrumb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb {
    /// label
    pub label: String,
    /// library
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_id: Option<String>,
    /// folder
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

/// Writable location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirTarget {
    /// folder
    pub folder_id: String,
    /// directory
    pub dir: String,
}

/// A file (folder + dir + name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLocation {
    /// folder
    pub folder_id: String,
    /// directory
    pub dir: String,
    /// file name
    pub name: String,
}

/// `GET /api/browse`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseResult {
    /// breadcrumbs
    pub crumbs: Vec<Crumb>,
    /// sub folders
    pub folders: Vec<SubFolder>,
    /// folders of a multi-folder library root
    pub roots: Vec<FolderRef>,
    /// page of items
    pub items: Vec<MediaItem>,
    /// total items
    pub total: i64,
    /// peekable archives here
    pub archives: Vec<MediaItem>,
    /// drop / upload target
    pub target: Option<DirTarget>,
    /// skeleton generation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gen: Option<String>,
}

/// Kind counts of a skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KindCounts {
    /// images
    pub image: i64,
    /// videos
    pub video: i64,
    /// audio
    pub audio: i64,
}

/// Day group of a skeleton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkeletonDay {
    /// 'YYYY-MM-DD'
    pub key: String,
    /// consecutive items of that day
    pub count: i64,
    /// dominant place label
    pub place: Option<String>,
}

/// `GET /api/browse/skeleton` — exact layout skeleton of a browse query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseSkeleton {
    /// changes whenever the result set changes
    pub gen: String,
    /// items
    pub total: i64,
    /// counts per kind
    pub kind_counts: KindCounts,
    /// base64 of one aspect byte per item ([`byte_to_aspect`])
    pub aspects: String,
    /// day groups (only when sorted by takenAt)
    pub days: Option<Vec<SkeletonDay>>,
}

/// Quantise an aspect ratio (w/h) into one byte: log scale over \[1/8, 8\].
pub fn aspect_to_byte(aspect: f64) -> u8 {
    let a = if aspect.is_finite() && aspect > 0.0 {
        aspect.clamp(1.0 / 8.0, 8.0)
    } else {
        1.0
    };
    js::round((a.log2() + 3.0) / 6.0 * 255.0) as u8
}

/// Inverse of [`aspect_to_byte`].
pub fn byte_to_aspect(b: u8) -> f64 {
    2f64.powf(b as f64 / 255.0 * 6.0 - 3.0)
}

/// Timeline month.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineBucket {
    /// YYYY-MM
    pub key: String,
    /// items
    pub count: i64,
}

// ───────────────────────── Server info & settings ─────────────────────────

/// How the server runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    /// Docker / headless
    Server,
    /// desktop app
    Desktop,
    /// Android local backend
    AndroidLocal,
}

/// Trash behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrashMode {
    /// OS trash
    Os,
    /// `.trash` folder
    Folder,
    /// none
    None,
}

/// Server capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// uploads
    pub upload: bool,
    /// move / copy / delete / rename
    pub file_ops: bool,
    /// native folder picker (desktop)
    pub native_folders: bool,
    /// server-side directory browser
    pub browse_host: bool,
    /// video edit jobs
    pub video_jobs: bool,
    /// slideshow export
    pub slideshow_export: bool,
    /// zip download
    pub zip_download: bool,
    /// trash behaviour
    pub trash: TrashMode,
    /// read-only server
    pub readonly: bool,
    /// LAN share
    pub lan_share: bool,
}

/// LAN share state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanInfo {
    /// enabled
    pub enabled: bool,
    /// reachable urls
    pub urls: Vec<String>,
}

/// `GET /api/info`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// app name
    pub name: String,
    /// app version
    pub version: String,
    /// API version
    pub api_version: u32,
    /// run mode
    pub mode: RunMode,
    /// host name
    pub hostname: String,
    /// a scan is running
    pub scanning: bool,
    /// capabilities
    pub caps: Capabilities,
    /// LAN share
    pub lan: LanInfo,
}

/// What to do with dropped zips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ZipAction {
    /// ask
    #[default]
    Ask,
    /// extract
    Extract,
    /// temp session
    Temp,
}

/// Settings.mapTiles: a tile style, 'off', or the legacy 'carto' (read as 'osm').
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MapTilesSetting {
    /// OpenStreetMap
    #[default]
    Osm,
    /// OpenTopoMap
    Topo,
    /// Esri imagery
    Satellite,
    /// no map
    Off,
    /// legacy value (= osm)
    Carto,
}

impl MapTilesSetting {
    /// tile style (legacy 'carto' → osm), None when off
    pub fn style(self) -> Option<MapTileStyle> {
        match self {
            MapTilesSetting::Osm | MapTilesSetting::Carto => Some(MapTileStyle::Osm),
            MapTilesSetting::Topo => Some(MapTileStyle::Topo),
            MapTilesSetting::Satellite => Some(MapTileStyle::Satellite),
            MapTilesSetting::Off => None,
        }
    }
}

/// Thumbnail WebP quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThumbQuality {
    /// low
    Low,
    /// normal
    #[default]
    Normal,
    /// high
    High,
}

/// Background indexer intensity.
pub type IndexIntensity = ThumbQuality;

/// `GET /api/settings`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// zip handling
    pub zip_action: ZipAction,
    /// IANA zone for UTC-only timestamps
    pub home_time_zone: String,
    /// map tiles
    pub map_tiles: MapTilesSetting,
    /// tile cache cap, MB
    pub tile_cache_mb: i64,
    /// edited copy suffix
    pub edit_suffix: String,
    /// show hidden files
    pub show_hidden: bool,
    /// temp session lifetime
    pub temp_session_hours: i64,
    /// host browser roots (server only)
    pub host_browse_roots: Vec<String>,
    /// remote stream cache cap, MB
    pub remote_cache_mb: i64,
    /// thumbnail quality
    pub thumb_quality: ThumbQuality,
    /// indexer intensity
    pub index_intensity: IndexIntensity,
    /// ffmpeg -hwaccel auto
    pub hw_accel: bool,
}

impl Default for Settings {
    /// `DEFAULT_SETTINGS`
    fn default() -> Self {
        Settings {
            zip_action: ZipAction::Ask,
            home_time_zone: "UTC".into(),
            map_tiles: MapTilesSetting::Osm,
            tile_cache_mb: 512,
            edit_suffix: "_edit".into(),
            show_hidden: false,
            temp_session_hours: 24,
            host_browse_roots: vec![],
            remote_cache_mb: 2048,
            thumb_quality: ThumbQuality::Normal,
            index_intensity: ThumbQuality::Normal,
            hw_accel: false,
        }
    }
}

// ───────────────────────── Uploads / zip ─────────────────────────

/// Zip handling of uploads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadZipMode {
    /// extract
    Extract,
    /// temp session
    Temp,
    /// keep the zip
    Keep,
}

/// Upload result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    /// created files
    pub created: Vec<FileLocation>,
    /// temp session folder created for a zip in temp mode
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_folder: Option<FolderRef>,
    /// extracted directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extracted_dir: Option<String>,
}

// ───────────────────────── Fonts ─────────────────────────

/// Font style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontStyle {
    /// upright
    Normal,
    /// italic
    Italic,
}

/// Font file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontFormat {
    /// ttf
    Truetype,
    /// otf
    Opentype,
    /// woff
    Woff,
    /// woff2
    Woff2,
}

/// A font.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontInfo {
    /// id
    pub id: String,
    /// family
    pub family: String,
    /// e.g. "Regular", "Bold Italic"
    pub subfamily: String,
    /// css weight
    pub weight: i64,
    /// style
    pub style: FontStyle,
    /// format
    pub format: FontFormat,
    /// file url
    pub url: String,
    /// shipped with the app
    pub bundled: bool,
}

// ───────────────────────── Edits & jobs ─────────────────────────

/// `POST /api/edits/save` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveEditResult {
    /// saved file
    pub item: FileLocation,
}

/// Colour adjustments (all optional: `Partial<VideoAdjust>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VideoAdjust {
    /// -1..1
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js::num_opt")]
    pub brightness: Option<f64>,
    /// -1..1
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js::num_opt")]
    pub contrast: Option<f64>,
    /// -1..1
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js::num_opt")]
    pub saturation: Option<f64>,
    /// degrees -180..180
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js::num_opt")]
    pub hue: Option<f64>,
    /// -1..1
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js::num_opt")]
    pub warmth: Option<f64>,
    /// 0..1
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js::num_opt")]
    pub vignette: Option<f64>,
}

/// Image overlay of a video edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoOverlay {
    /// uploaded job asset (transparent PNG at output resolution)
    pub asset_id: String,
    /// seconds on the OUTPUT timeline
    #[serde(with = "js::num")]
    pub start: f64,
    /// seconds on the OUTPUT timeline
    #[serde(with = "js::num")]
    pub end: f64,
}

/// A time range in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    /// seconds
    #[serde(with = "js::num")]
    pub start: f64,
    /// seconds
    #[serde(with = "js::num")]
    pub end: f64,
}

/// Crop rectangle in source pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CropRect {
    /// left
    #[serde(with = "js::num")]
    pub x: f64,
    /// top
    #[serde(with = "js::num")]
    pub y: f64,
    /// width
    #[serde(with = "js::num")]
    pub w: f64,
    /// height
    #[serde(with = "js::num")]
    pub h: f64,
}

/// Rotation in degrees (`0 | 90 | 180 | 270`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub enum Rotation {
    /// 0°
    R0,
    /// 90°
    R90,
    /// 180°
    R180,
    /// 270°
    R270,
}

impl TryFrom<u16> for Rotation {
    type Error = String;
    fn try_from(v: u16) -> Result<Self, String> {
        Ok(match v {
            0 => Rotation::R0,
            90 => Rotation::R90,
            180 => Rotation::R180,
            270 => Rotation::R270,
            _ => return Err(format!("invalid rotation {v}")),
        })
    }
}

impl From<Rotation> for u16 {
    fn from(r: Rotation) -> u16 {
        match r {
            Rotation::R0 => 0,
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }
}

/// Video export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    /// H.264 mp4
    Mp4,
    /// VP9 webm
    Webm,
    /// animated gif
    Gif,
}

/// Export quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoQuality {
    /// high
    High,
    /// medium
    Medium,
    /// low
    Low,
}

/// Output of a video edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoOutput {
    /// format
    pub format: VideoFormat,
    /// target height in px, None = keep
    pub height: Option<i64>,
    /// quality
    pub quality: VideoQuality,
}

/// `POST /api/jobs/video-edit`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoEditSpec {
    /// source media id
    pub source_id: String,
    /// trim range
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<TimeRange>,
    /// crop rectangle (source pixels)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<CropRect>,
    /// rotation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<Rotation>,
    /// mirror
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_h: Option<bool>,
    /// flip
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip_v: Option<bool>,
    /// 0.25 .. 4
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js::num_opt")]
    pub speed: Option<f64>,
    /// 0 .. 2
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js::num_opt")]
    pub volume: Option<f64>,
    /// drop audio
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    /// colour adjustments
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjust: Option<VideoAdjust>,
    /// overlays
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlays: Option<Vec<VideoOverlay>>,
    /// output
    pub output: VideoOutput,
}

/// Slideshow transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlideTransition {
    /// cut
    None,
    /// cross fade
    Fade,
    /// slide
    Slide,
    /// zoom
    Zoom,
    /// Ken Burns
    Kenburns,
    /// blur
    Blur,
    /// flip
    Flip,
    /// circle
    Circle,
    /// random
    Random,
}

/// One slide of a slideshow export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlideshowItem {
    /// media id
    pub id: String,
    /// seconds
    #[serde(default, skip_serializing_if = "Option::is_none", with = "js::num_opt")]
    pub duration: Option<f64>,
    /// video trim
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<TimeRange>,
}

/// `POST /api/jobs/slideshow`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlideshowExportSpec {
    /// slides
    pub items: Vec<SlideshowItem>,
    /// seconds per image
    #[serde(with = "js::num")]
    pub duration: f64,
    /// transition
    pub transition: SlideTransition,
    /// seconds
    #[serde(with = "js::num")]
    pub transition_duration: f64,
    /// Ken Burns effect
    pub ken_burns: bool,
    /// background music
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "js::double_option"
    )]
    pub audio_id: Option<Option<String>>,
    /// px
    pub width: i64,
    /// px
    pub height: i64,
    /// frames per second
    pub fps: i64,
    /// hex
    pub background: String,
    /// where to save (default: folder of the first item)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<DirTarget>,
    /// file name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Job state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// waiting
    Queued,
    /// running
    Running,
    /// finished
    Done,
    /// failed
    Error,
    /// canceled
    Canceled,
}

/// Job type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobType {
    /// video edit
    VideoEdit,
    /// slideshow export
    Slideshow,
    /// file transfer
    Transfer,
}

/// done / total counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Progress {
    /// done
    pub done: i64,
    /// total
    pub total: i64,
}

/// A background job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    /// id
    pub id: String,
    /// type
    #[serde(rename = "type")]
    pub kind: JobType,
    /// title
    pub title: String,
    /// state
    pub status: JobStatus,
    /// 0..1
    #[serde(with = "js::num")]
    pub progress: f64,
    /// transfers: bytes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Progress>,
    /// transfers: files
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Progress>,
    /// error message
    pub error: Option<String>,
    /// produced file
    pub result: Option<FileLocation>,
    /// ms epoch
    pub created_at: i64,
}

// ───────────────────────── Realtime events ─────────────────────────

/// Why media changed (`media` event).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaReason {
    /// structure changed (refetch skeleton)
    Add,
    /// structure changed (refetch skeleton)
    Remove,
    /// file changed
    Change,
    /// dates/dimensions changed (layout may change)
    Meta,
    /// only thumbhash/thumbnail ready (patch in place)
    Thumb,
}

/// WebSocket / SSE event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerEvent {
    /// first message
    Hello {
        /// server info
        info: ServerInfo,
    },
    /// media of a folder changed
    Media {
        /// folder
        folder_id: String,
        /// changed dirs
        dirs: Vec<String>,
        /// reason
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<MediaReason>,
    },
    /// device list / online state changed
    Devices,
    /// a phone asks to be paired
    Pair {
        /// the phone
        device: DeviceInfo,
    },
    /// grouping data changed
    Explore,
    /// libraries / folders / temp sessions changed
    Folders {
        /// only the state shown for a folder changed
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<bool>,
    },
    /// albums or their items changed
    Albums {
        /// changed albums
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ids: Option<Vec<String>>,
    },
    /// scan progress
    Scan {
        /// scanning
        scanning: bool,
        /// done
        done: i64,
        /// total
        total: i64,
    },
    /// job update
    Job {
        /// the job
        job: Job,
    },
    /// fonts changed
    Fonts,
    /// the MEGA account session changed
    MegaAccount,
}

// ───────────────────────── File operations ─────────────────────────

/// A file inside a folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    /// folder
    pub folder_id: String,
    /// path relative to folder root (no leading slash)
    pub rel: String,
}

/// Host directory browser entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDirEntry {
    /// name
    pub name: String,
    /// absolute path
    pub path: String,
}

// ───────────────────────── Explore (automatic grouping) ─────────────────────────

/// MEDIA_FLAG bits.
pub mod media_flag {
    /// screenshots
    pub const SCREENSHOT: u32 = 1;
    /// selfies
    pub const SELFIE: u32 = 2;
    /// panoramas
    pub const PANORAMA: u32 = 4;
    /// animated images
    pub const ANIMATED: u32 = 8;
}

/// `keyof MEDIA_FLAG | 'video'`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaFlagName {
    /// screenshot
    Screenshot,
    /// selfie
    Selfie,
    /// panorama
    Panorama,
    /// animated
    Animated,
    /// videos (a kind, not a flag)
    Video,
}

impl MediaFlagName {
    /// MEDIA_FLAG bit (None for 'video')
    pub fn bit(self) -> Option<u32> {
        match self {
            MediaFlagName::Screenshot => Some(media_flag::SCREENSHOT),
            MediaFlagName::Selfie => Some(media_flag::SELFIE),
            MediaFlagName::Panorama => Some(media_flag::PANORAMA),
            MediaFlagName::Animated => Some(media_flag::ANIMATED),
            MediaFlagName::Video => None,
        }
    }
}

/// Places group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceGroup {
    /// PlaceInfo.key
    pub key: String,
    /// country
    pub cc: String,
    /// admin1 name, or the city when admin1 is unknown
    pub name: String,
    /// most common city inside the group
    pub city: Option<String>,
    /// items
    pub count: i64,
    /// cover
    pub cover_id: String,
    /// centre
    #[serde(with = "js::num")]
    pub lat: f64,
    /// centre
    #[serde(with = "js::num")]
    pub lon: f64,
}

/// Camera group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraGroup {
    /// normalised "make|model"
    pub key: String,
    /// make
    pub make: Option<String>,
    /// model
    pub model: String,
    /// items
    pub count: i64,
    /// cover
    pub cover_id: String,
}

/// Media type group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeGroup {
    /// flag
    pub flag: MediaFlagName,
    /// items
    pub count: i64,
    /// cover
    pub cover_id: String,
}

/// Year group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YearGroup {
    /// year
    pub year: i64,
    /// items
    pub count: i64,
    /// cover
    pub cover_id: String,
}

/// Memory kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    /// on this day
    Onthisday,
    /// trip
    Trip,
    /// year in review
    Year,
}

/// A memory (on this day / trip / year in review).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    /// id
    pub id: String,
    /// kind
    pub kind: MemoryKind,
    /// onthisday: whole years ago
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub years_ago: Option<i64>,
    /// trip: place label
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// trip: country
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    /// localAt range start
    pub from: i64,
    /// localAt range end (exclusive)
    pub to: i64,
    /// items
    pub count: i64,
    /// covers
    pub cover_ids: Vec<String>,
    /// year in review: items per month
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub months: Option<Vec<i64>>,
}

/// `GET /api/explore`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreResult {
    /// insights (stats, year reviews, countries …)
    #[serde(flatten)]
    pub extras: ExploreExtras,
    /// memories
    pub memories: Vec<Memory>,
    /// places
    pub places: Vec<PlaceGroup>,
    /// types
    pub types: Vec<TypeGroup>,
    /// cameras
    pub cameras: Vec<CameraGroup>,
    /// years
    pub years: Vec<YearGroup>,
    /// background metadata enrichment progress
    pub indexing: Progress,
}

/// One map point `[id, lat, lon]`.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoPoint(pub String, pub f64, pub f64);

impl Serialize for GeoPoint {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (&self.0, js::JsF64(self.1), js::JsF64(self.2)).serialize(s)
    }
}

impl<'de> Deserialize<'de> for GeoPoint {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (id, lat, lon) = <(String, f64, f64)>::deserialize(d)?;
        Ok(GeoPoint(id, lat, lon))
    }
}

/// `GET /api/geo/points`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoPoints {
    /// points
    pub points: Vec<GeoPoint>,
}

// ───────────────────────── Linked devices (phone ↔ PC) ─────────────────────────

/// What a phone allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCaps {
    /// list / copy from
    pub read: bool,
    /// copy into its folders
    pub receive: bool,
}

/// A linked phone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// id
    pub id: String,
    /// name
    pub name: String,
    /// model
    pub model: String,
    /// connected
    pub online: bool,
    /// approved by the PC user
    pub paired: bool,
    /// ms epoch
    pub last_seen: i64,
    /// shared folders
    pub folders: Vec<FolderRef>,
    /// pending pairing only: 4-digit code
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// what the phone allows
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caps: Option<DeviceCaps>,
}

// ───────────────────────── Remote links (MEGA) ─────────────────────────

/// A node of a shared MEGA tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteNode {
    /// node id
    pub id: String,
    /// '/' separated path inside the shared tree
    pub path: String,
    /// name
    pub name: String,
    /// bytes
    pub size: i64,
    /// directory
    pub dir: bool,
    /// media kind
    pub kind: Option<MediaKind>,
}

/// Cloud provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteProvider {
    /// mega.nz
    Mega,
}

/// File or folder link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    /// single file link
    File,
    /// folder link
    Folder,
}

/// `POST /api/remote/inspect`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteInspect {
    /// provider
    pub provider: RemoteProvider,
    /// link kind
    pub kind: LinkKind,
    /// name
    pub name: String,
    /// bytes
    pub size: i64,
    /// nodes
    pub nodes: Vec<RemoteNode>,
    /// node list cut
    pub truncated: bool,
}

/// Import mode of a remote link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteImportMode {
    /// into folderId/dir
    Extract,
    /// a new temp session
    Temp,
}

/// `POST /api/remote/import`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteImportSpec {
    /// link
    pub url: String,
    /// node ids to fetch; omitted = everything
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<String>>,
    /// mode
    pub mode: RemoteImportMode,
    /// target folder
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// target dir
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// zips inside the link
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zip: Option<UploadZipMode>,
}

// ───────────────────────── Route response wrappers ─────────────────────────

/// `GET /api/libraries`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibrariesResponse {
    /// libraries
    pub libraries: Vec<Library>,
    /// temp sessions (zip peeks)
    pub temp: Vec<FolderRef>,
    /// linked phones (only for viewers allowed to see them)
    pub devices: Vec<DeviceInfo>,
}

/// `GET /api/albums`, `POST /api/albums/order`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumsResponse {
    /// albums
    pub albums: Vec<crate::albums::Album>,
}

/// `GET /api/jobs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobsResponse {
    /// jobs
    pub jobs: Vec<Job>,
}

/// `GET /api/fonts`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontsResponse {
    /// fonts
    pub fonts: Vec<FontInfo>,
}

/// `GET /api/timeline`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineResponse {
    /// months, newest first
    pub buckets: Vec<TimelineBucket>,
}

/// `GET /api/search`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    /// matches
    pub items: Vec<MediaItem>,
    /// total matches
    pub total: i64,
}

/// `GET /api/explore/duplicates`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicatesResponse {
    /// groups of identical files
    pub groups: Vec<crate::insights::DuplicateGroup>,
    /// total groups
    pub total: i64,
    /// the items of the groups
    pub items: Vec<MediaItem>,
    /// candidates still waiting to be hashed
    pub pending: i64,
    /// media id → "folder name/dir"
    pub paths: IndexMap<String, String>,
}

/// `POST /api/zip/peek`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZipPeekResponse {
    /// the new temp session
    pub temp_folder: FolderRef,
}

/// `GET /api/devices`
pub type DevicesResponse = Vec<DeviceInfo>;

// ───────────────────────── Helpers ─────────────────────────

/// Image extensions.
pub const IMAGE_EXT: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "avif", "bmp", "svg", "heic", "heif", "tif", "tiff",
    "jfif", "ico",
];
/// Images browsers show natively.
pub const BROWSER_IMAGE_EXT: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "gif", "webp", "avif", "bmp", "svg", "ico",
];
/// Video extensions.
pub const VIDEO_EXT: &[&str] = &[
    "mp4", "m4v", "mov", "webm", "mkv", "avi", "wmv", "flv", "mpg", "mpeg", "3gp", "ts", "mts",
    "m2ts", "ogv",
];
/// Audio extensions.
pub const AUDIO_EXT: &[&str] = &["mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "flac"];
/// Archive extensions.
pub const ARCHIVE_EXT: &[&str] = &["zip"];
/// Font extensions.
pub const FONT_EXT: &[&str] = &["ttf", "otf", "woff", "woff2"];

/// lower-case extension without dot ('' for none / dot files)
pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// media kind by extension
pub fn kind_of(name: &str) -> Option<MediaKind> {
    let e = ext_of(name);
    let e = e.as_str();
    if IMAGE_EXT.contains(&e) {
        Some(MediaKind::Image)
    } else if VIDEO_EXT.contains(&e) {
        Some(MediaKind::Video)
    } else if AUDIO_EXT.contains(&e) {
        Some(MediaKind::Audio)
    } else if ARCHIVE_EXT.contains(&e) {
        Some(MediaKind::Archive)
    } else {
        None
    }
}

/// `dir/name` ('' dir → name)
pub fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// parent dir of a relative path ('' for top level)
pub fn parent_dir(rel: &str) -> &str {
    match rel.rfind('/') {
        Some(i) => &rel[..i],
        None => "",
    }
}
