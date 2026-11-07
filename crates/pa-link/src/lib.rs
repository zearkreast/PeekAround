//! PeekAround phone link (mirrors `packages/server/src/devices/*` + `routes/devices.ts`,
//! protocol in docs/link-protocol.md, wire-compatible with the 1.3.x Android app):
//!
//! - [`hub`]: [`DeviceHub`] — WebSocket control plane (hello / req / cancel / res / err / folders /
//!   changed / ping), 128-bit rids, reply-PUT data plane with backpressure, per-device in-flight
//!   limits and priorities, first-byte timeouts, pairing, folder rows, listing → media rows,
//!   background thumbnails, one-time pull URLs
//! - [`access`]: device visibility rules, approver rules, `?pdev=&ptok=`
//! - [`media`]: ranged `/file` through the tunnel, `details`, loopback ffmpeg input
//! - [`transfer`]: copy device → PC / PC → device transfer jobs, pull GET
//! - [`store`]: the traits the server implements; [`axum`] (feature): ready-made glue
//!
//! See `API.md`.

pub mod access;
#[cfg(feature = "axum")]
pub mod axum;
pub mod hub;
pub mod media;
pub mod ndjson;
pub mod store;
pub mod transfer;

pub use access::{RequestInfo, RunMode, Viewer};
pub use hub::{ConnectMeta, DeviceHub, HubDeps, ReqOpts, SocketHandle, TunnelReply, WsOut};
pub use store::{DeviceFolderRow, DeviceRow, DeviceStore};
pub use transfer::{HostFolder, TransferHost, Transfers};
