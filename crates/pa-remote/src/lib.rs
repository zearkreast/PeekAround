//! PeekAround MEGA streaming folders (mirrors `packages/server/src/remote/{folders,cache,import}.ts`,
//! the account glue of `remote/account.ts` and the logic of `routes/remote.ts`) on top of pa-mega.
//!
//! - [`service`]: [`RemoteService`] — add a link as a folder, listing / refresh diff, status
//!   (listing / ok / offline / quota / error + retry schedule), background metadata and thumbnails,
//!   ranged reads, thumbnail fallback chain, zip entries, copy to this computer, shutdown
//! - [`cache`]: [`RemoteCache`] — the size-capped LRU disk cache of viewed originals
//! - [`import`]: [`Importer`] — the MEGA import / copy transfer job
//! - [`account`]: [`RemoteAccount`] — the optional MEGA account session wired to the service
//! - [`routes`]: the `/api/remote/*` route logic (bodies as raw JSON like the TS validators)
//! - [`store`] / [`backend`]: the traits the server implements
//!
//! See `API.md` for the route mapping.

pub mod account;
pub mod backend;
pub mod cache;
pub mod import;
pub mod routes;
pub mod service;
pub mod store;

pub use account::RemoteAccount;
pub use backend::MegaBackend;
pub use cache::RemoteCache;
pub use import::{ImportOptions, Importer};
pub use pa_mega::Priority;
pub use service::{
    abs_of, details_path, details_video, error_code, link_is_local, RemoteDeps, RemoteService,
    RemoteSpec, Tree, META_REMOTE_PENDING, REMOTE_READONLY, REMOTE_REFRESH_MS,
};
pub use store::{ImportHost, NewRemoteFolder, RemoteFolderRow, RemoteMedia, RemoteStore};

use pa_jobs::HttpError;
use pa_mega::MegaError;

/// pa-mega's HTTP error → the server's
pub(crate) fn hx(e: pa_mega::HttpError) -> HttpError {
    HttpError {
        status: e.status,
        message: e.message,
        retry_after: e.retry_after,
    }
}

/// `megaError(e)`: friendly HTTP errors for MEGA failures (an HTTP error stays as it is)
pub fn mega_error(e: &MegaError) -> HttpError {
    hx(pa_mega::http_error(e))
}

/// an HTTP error as a MEGA error (so it survives `mega_error` unchanged)
pub(crate) fn to_mega(e: HttpError) -> MegaError {
    MegaError::Http4xx(pa_mega::HttpError {
        status: e.status,
        message: e.message,
        retry_after: e.retry_after,
    })
}

/// JS truthiness of a JSON value
pub(crate) fn truthy(v: &serde_json::Value) -> bool {
    use serde_json::Value;
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        _ => true,
    }
}
