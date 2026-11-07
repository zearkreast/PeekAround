//! Remote (MEGA streaming) folders: the server side of `remote/folders.ts` — a thin facade over the
//! pa-remote engine ([`pa_remote::RemoteService`] + [`pa_remote::RemoteAccount`]); the database /
//! scanner / thumbnail side is [`crate::engines::Store`].

use crate::ctx::{Ctx, C};
use crate::db::{FolderRow, MediaRow};
use crate::error::ApiResult;
use pa_core::{MediaDetails, RemoteFolderInfo};
use std::path::PathBuf;
use std::sync::Arc;

pub use pa_remote::META_REMOTE_PENDING;

/// The service (`ctx.remote`).
pub struct RemoteService {
    /// `<cache>/remote`
    pub cache_dir: PathBuf,
    pub svc: Arc<pa_remote::RemoteService>,
    /// the optional MEGA account (`ctx.megaAccount`)
    pub account: pa_remote::RemoteAccount,
}

/// `folders` row → the engine's row (None: not a MEGA folder)
pub fn row_of(folder: &FolderRow) -> Option<pa_remote::RemoteFolderRow> {
    Some(pa_remote::RemoteFolderRow {
        id: folder.id,
        name: folder.name.clone(),
        path: folder.path.clone(),
        remote: folder.remote.clone()?,
        remote_status: folder.remote_status.clone(),
    })
}

impl RemoteService {
    /// FolderRef.remote — never contains the key
    pub fn info(&self, folder: &FolderRow) -> Option<RemoteFolderInfo> {
        self.svc.info(&row_of(folder)?).ok()
    }

    /// [`RemoteService::info`] (the engine adds the indexing counts)
    pub fn info_with_counts(&self, _ctx: &Ctx, folder: &FolderRow) -> Option<RemoteFolderInfo> {
        self.info(folder)
    }

    /// the full link (with key)
    pub fn link_of(&self, folder: &FolderRow) -> ApiResult<String> {
        let row = row_of(folder)
            .ok_or_else(|| crate::error::HttpError::new(400, "Not a remote folder"))?;
        Ok(self.svc.link_of(&row)?)
    }

    /// re-list a remote folder in the background (`scanner.remoteScan`)
    pub fn scan(&self, _ctx: &C, folder_id: i64) {
        // a shared future that runs even when dropped
        drop(self.svc.refresh(folder_id));
    }

    /// background work after a cache clear
    pub fn kick(&self) {
        self.svc.kick();
    }

    /// forget a removed folder
    pub fn forget(&self, id: i64) {
        self.svc.forget(id);
    }

    /// bytes of the stream cache
    pub async fn cache_size(&self) -> u64 {
        self.svc.cache.size_bytes().await
    }

    /// clear the stream cache
    pub async fn cache_clear(&self) {
        self.svc.cache.clear().await;
    }

    /// MediaDetails of a remote row (from the index; `exif: null`)
    pub fn details(&self, ctx: &Ctx, r: &MediaRow, folder: &FolderRow) -> MediaDetails {
        let mut d = crate::routes::media::details_from_row(
            ctx,
            r,
            pa_remote::details_path(&folder.name, &r.dir, &r.name),
            None,
        );
        d.video = pa_remote::details_video(crate::db::kind_of_str(&r.kind), r.extra.as_deref());
        d
    }
}
