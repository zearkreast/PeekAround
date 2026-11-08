//! The phone side of the phone ↔ PC folder sharing (PeekAround Link v1, docs/link-protocol.md;
//! the server side is `pa-link`). Port of the 1.x Android app's `Link*.kt` / `PeekMediaServer`:
//! [`LinkClient`] keeps one WebSocket open to the PC and answers its requests from the shared
//! folders; OS services (video frames, MediaStore thumbnails, media scanner, notification state)
//! come from a [`Platform`]. Platform-independent, so it is tested on the desktop against the
//! real pa-server.

pub mod client;
pub mod codec;
pub mod ops;
pub mod prefs;
pub mod watch;

pub use client::{LinkClient, LinkEnd};
pub use ops::{NoPlatform, Platform, VideoInfo};
pub use prefs::{LinkFolder, LinkPrefs, LinkStatus};
