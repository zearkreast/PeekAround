//! PeekAround metadata extraction (mirrors `packages/server/src/media/metadata.ts`).
//!
//! - [`extract_meta`]: one file → [`ExtractedMeta`] (EXIF / XMP for images, a [`pa_core::ProbeResult`]
//!   for videos, Google Takeout sidecars for what the file lacks)
//! - [`group_columns`]: [`ExtractedMeta`] → DB grouping columns (time zone resolution, reverse geocoding,
//!   camera normalisation, MEDIA_FLAG classification)
//! - [`read_exif`]: the picked EXIF tags of the info panel
//!
//! The EXIF / XMP readers reproduce exifr 7 (what the TypeScript server used) so the extracted values are
//! identical — see `PARITY.md`. Everything is pure Rust; readers work on any [`Source`] (bytes or files).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod dims;
pub mod exifr;
mod meta;
pub mod source;
mod tags;
mod tiff;
pub mod value;
mod xmp;

pub use meta::*;
pub use source::{ReaderSource, Source};
pub use value::{Tags, Val};
