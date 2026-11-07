//! PeekAround media engine (mirrors the image/video side of the TypeScript server):
//!
//! - [`decode`]: every still format → upright RGB(A) (mozjpeg, image, libheif, resvg)
//! - [`info`]: header reads like `sharp.metadata()` (size, pages, orientation)
//! - [`resize`] / [`encode`]: sharp geometry + Lanczos3, WebP / JPEG / PNG encoders
//! - [`thumbhash`]: byte-identical port of the npm `thumbhash` package
//! - [`render`]: the thumbnail / display renditions of `thumbs.ts` + cache file helpers
//! - [`ffmpeg`] / [`jobs`] / [`video`] / [`process`]: ffmpeg CLI wrapper, `ffmpeg -i` parser,
//!   export-job process plumbing, preview / filmstrip, low-priority child processes
//! - [`concurrency`]: decode limiter, background pool, `indexIntensity` budget

pub mod concurrency;
pub mod decode;
pub mod encode;
pub mod error;
pub mod exif;
pub mod ffmpeg;
pub mod icc;
pub mod info;
pub mod jobs;
pub mod process;
pub mod render;
pub mod resize;
pub mod thumbhash;
pub mod video;

pub use concurrency::{
    intensity_limits, BackgroundPool, IndexIntensity, IntensityLimits, Limiter, Permit, Priority,
};
pub use decode::{decode_bytes, decode_path, DecodeOptions};
pub use error::{MediaError, Result};
pub use ffmpeg::{Ffmpeg, ProbeOptions, ProbeResult};
pub use info::{image_info, image_info_path, Format, ImageInfo};
pub use render::{BaseRenditions, DisplayFormat, ThumbQuality};
