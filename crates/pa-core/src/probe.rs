//! `ProbeResult` — what ffprobe / `ffmpeg -i` tell about a video or audio file
//! (mirrors the interface in `packages/server/src/ffmpeg.ts`). Filled by `pa-media`, read by `pa-meta`.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::js;

/// Probe result of one media file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    /// seconds
    #[serde(with = "js::num_opt")]
    pub duration: Option<f64>,
    /// display width (rotation applied)
    pub width: Option<i64>,
    /// display height (rotation applied)
    pub height: Option<i64>,
    /// 0 / 90 / 180 / 270
    pub rotation: i32,
    /// video codec name
    pub codec: Option<String>,
    /// audio codec name
    pub audio_codec: Option<String>,
    /// frames per second (2 decimals)
    #[serde(with = "js::num_opt")]
    pub fps: Option<f64>,
    /// bits per second
    #[serde(with = "js::num_opt")]
    pub bitrate: Option<f64>,
    /// container (ffmpeg format name)
    pub container: Option<String>,
    /// `creation_time` tag as ms epoch
    pub creation_time: Option<i64>,
    /// has an audio stream
    pub has_audio: bool,
    /// container + video stream tags, lower-cased keys, in the order ffprobe / ffmpeg printed them
    /// (video stream tags first, container tags override — like `probeFromJson`)
    pub tags: IndexMap<String, String>,
}

/// Browser-playable video (`isBrowserPlayable`): mp4/m4v/mov/webm/ogv with h264/vp8/vp9/av1/theora.
pub fn is_browser_playable(ext: &str, codec: Option<&str>) -> bool {
    if !["mp4", "m4v", "mov", "webm", "ogv"].contains(&ext) {
        return false;
    }
    match codec.filter(|c| !c.is_empty()) {
        None => ext != "mov",
        Some(c) => ["h264", "vp8", "vp9", "av1", "theora"].contains(&c),
    }
}
