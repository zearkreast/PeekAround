//! PeekAround MEGA client (mirrors `remote/{mega,crypto,account}.ts` and the MEGA transfer parts of
//! `remote/{folders,import}.ts`): public file / folder links, folder trees with raw `fa`
//! attributes, AES-CTR ranged reads from any offset, MAC-checked downloads, thumbnails / previews
//! (`ufa`), download-URL cache, error mapping, retry / back-off, transfer lanes and quota pause, and
//! the optional account session (v1/v2 login, 2FA, `ug` / `uq`, `sml`, ESID handling).
//!
//! Pure Rust + rustls: builds for Windows, Linux, Android.

pub mod account;
pub mod api;
pub mod client;
pub mod crypto;
pub mod error;
pub mod import;
pub mod link;
pub mod transfer;
pub mod tree;

pub use account::{
    AccountStatus, AuthClient, LoginOutcome, LoginRefusal, LoginRequest, LoginResult, MegaAccount,
    MegaAuth, SessionStore, StoredSession,
};
pub use api::{Api, ApiOptions, SessionSource, DEFAULT_GATEWAY};
pub use client::{collect, ByteStream, MegaClient, MegaNode};
pub use error::{http_error, HttpError, MegaError};
pub use link::{parse_mega_url, LinkType, MegaLink, SubNode, SubType};
pub use transfer::{Lanes, Priority, TransferGate};
pub use tree::{flatten, inspect_tree, remote_paths, RemoteInspect};
