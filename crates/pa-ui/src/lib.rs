//! PeekAround Slint UI: every screen + its view model, independent of where the data comes from
//! (a [`backend::Backend`]: the HTTP API via `pa-client`, later Android's MediaStore).
//!
//! Renderer: femtovg (OpenGL / GLES through glutin) on the winit backend — see README.md for why.
//! The binary owns the tokio runtime and passes its handle; Slint owns the UI thread.

#![warn(missing_docs)]
#![allow(clippy::too_many_arguments)]

pub mod backend;
mod files;
pub mod fmt;
pub mod host;
pub mod grid;
pub mod i18n;
pub mod perf;
pub mod prefs;
pub mod rt;
pub mod theme;
pub mod thumbs;
// viewer, video, selection, actions, drag & drop
pub mod actions;
pub mod actions_ops;
pub mod actions_ui;
mod actions_script;
pub mod selection;
#[cfg(not(target_arch = "wasm32"))]
pub mod video;
#[cfg(target_arch = "wasm32")]
#[path = "video_web.rs"]
pub mod video;
mod video_common;
#[cfg(target_arch = "wasm32")]
pub mod web;
mod viewer;
mod viewer_fmt;

// Explore pages, memory player, map, mini map
mod explore;
mod explore_countries;
mod explore_flags;
mod explore_labels;
mod explore_player;
mod map;
pub mod map_cluster;
pub mod map_math;

mod api;
mod app;
mod pages;
mod router;
mod slideshow;
mod slideshow_show;
mod slideshow_ui;
mod editor;
mod video_editor;
mod collage;
mod media_picker;

#[allow(missing_docs, clippy::all)]
mod generated {
    slint::include_modules!();
}
pub use generated::*;

pub use app::{run, run_gallery, run_with, Integration, UiOptions};
pub use pages::announce_update;
#[cfg(any(feature = "desktop", feature = "web"))]
pub use pages::observe_window_events;
