//! The MEGA side of the import job (`downloadOne` of remote/import.ts): stream one file into a
//! `.part` next to its destination, with progress, the node's timestamp as mtime, and no leftover
//! on failure. Unique naming, indexing and zip handling stay in the server.

use crate::client::{MegaClient, MegaNode};
use crate::error::MegaError;
use crate::link::MegaLink;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// latest mtime the TS code accepts (2100-01-01)
const MAX_TS: i64 = 4_102_444_800_000;

/// Download `node` into `<dir>/<name>.<random>.part` (MAC verified); returns the part path.
pub async fn download_to_part(
    client: &MegaClient,
    link: &MegaLink,
    node: &MegaNode,
    dir: &Path,
    name: &str,
    cancel: CancellationToken,
    on_bytes: &mut (dyn FnMut(u64) + Send),
) -> Result<PathBuf, MegaError> {
    tokio::fs::create_dir_all(dir).await?;
    let part = dir.join(format!("{name}.{:08x}.part", rand::random::<u32>()));
    let r = async {
        let mut f = tokio::fs::File::create(&part).await?;
        if node.size > 0 {
            let mut s = client.download(link, node, cancel.clone())?;
            while let Some(c) = s.next().await {
                let c = c?;
                f.write_all(&c).await?;
                on_bytes(c.len() as u64);
            }
        }
        f.flush().await?;
        let f = f.into_std().await;
        if let Some(ts) = node.timestamp.filter(|t| *t > 0 && *t < MAX_TS) {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ts as u64);
            let _ = tokio::task::spawn_blocking(move || {
                f.set_times(
                    std::fs::FileTimes::new()
                        .set_modified(t)
                        .set_accessed(std::time::SystemTime::now()),
                )
            })
            .await;
        }
        Ok::<_, MegaError>(())
    }
    .await;
    match r {
        Ok(()) => Ok(part),
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}
