//! PeekAround job engine (mirrors `packages/server/src/jobs/*` + the `/stream` transcode of
//! `routes/media.ts`), independent of the HTTP server:
//!
//! - [`queue`]: [`JobService`] — two serial lanes (media exports / transfers), lifecycle, progress,
//!   cancel, shutdown, job assets (`ASSET_EXT`); events through [`EventSink`]
//! - [`video_edit`] / [`slideshow`]: pure spec validation + ffmpeg argument builders (golden-tested
//!   against the TypeScript builders)
//! - [`runners`]: [`Exporter`] — validate + probe + queue video edits and slideshow exports; the
//!   library side is the [`MediaLookup`] trait
//! - [`stream`]: live fragmented-MP4 transcode sessions, `hwAccel`, internal loopback inputs
//! - [`paths`] / [`error`] / [`events`]: helpers shared with pa-link and pa-remote
//!
//! See `API.md` for the route mapping.

// `!(x > 0.0)` and `.min().max()` are deliberate: they keep the JS NaN semantics of the TS code.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::manual_clamp)]

pub mod canvas;
pub mod error;
pub mod events;
pub mod js;
pub mod library;
pub mod paths;
pub mod queue;
pub mod runners;
pub mod slideshow;
pub mod spec;
pub mod stream;
pub mod video_edit;

pub use error::{HttpError, JobFailure};
pub use events::{EventSink, MemoryEvents, NoEvents};
pub use queue::{random_uuid, runner, JobContext, JobResult, JobRunner, JobService, ASSET_EXT};
pub use runners::{Exporter, JobMedia, MediaLookup};
