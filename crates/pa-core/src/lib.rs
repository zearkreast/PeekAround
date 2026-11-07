//! PeekAround core: the API contract (JSON types shared with the TypeScript server / web UI) and the
//! pure grouping logic of `packages/shared` — time zones, offline reverse geocoding, classification,
//! memories, trips, search parsing, Takeout sidecars, sun position, colours, insights, tiles, albums.
//!
//! No IO: everything takes values / bytes, so the crate builds for wasm32, Android and Windows.
//! Load the GeoNames asset (`packages/web/public/geo/cities.bin`) with [`decode_geo_asset`] from bytes.
//!
//! Parity with the TypeScript reference is checked by `tests/golden.rs` (12k generated cases from
//! `tests/tools/gen-core.ts`), `tests/api.rs` (141 real TS server responses must round-trip byte for
//! byte) and the ported unit tests. Deliberate differences:
//! - time zones come from jiff's bundled tz database instead of the JS engine's ICU data (equal on all
//!   tested instants, including DST rules beyond 2037; may drift until the crates are updated);
//! - `localeCompare` (city sorting) uses ICU4X root collation;
//! - [`homes_by_year`] returns a map sorted by year (a JS `Map` keeps insertion order);
//! - [`compute_insights`] with `ranked: 0` returns empty rankings (the TS version throws);
//! - JS regexps in [`ExcludeMatcher`] match per code point (JS: per UTF-16 unit) and `.` also matches `\r`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod albums;
pub mod classify;
pub mod color;
pub mod geo;
pub mod insights;
pub mod js;
pub mod memories;
pub mod probe;
pub mod remote_folders;
pub mod search;
pub mod sun;
pub mod takeout;
pub mod tiles;
pub mod time;
pub mod trips;
pub mod types;

pub use albums::*;
pub use classify::*;
pub use color::*;
pub use geo::*;
pub use insights::*;
pub use memories::*;
pub use probe::*;
pub use remote_folders::*;
pub use search::*;
pub use sun::*;
pub use takeout::*;
pub use tiles::*;
pub use time::*;
pub use trips::*;
pub use types::*;
