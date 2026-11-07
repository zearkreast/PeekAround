//! The MEGA operations the remote folders use (what `ctx.mega` / `MegaNode.read|download|attr` are
//! in the TS server). Implemented for [`pa_mega::MegaClient`]; tests wrap it to inject faults.

use async_trait::async_trait;
use pa_mega::{ByteStream, MegaClient, MegaError, MegaLink, MegaNode};
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait MegaBackend: Send + Sync + 'static {
    /// the link's node (file link) or whole tree (folder link)
    async fn load(&self, link: &MegaLink) -> Result<MegaNode, MegaError>;
    /// decrypted bytes `[start, end]` of a file node; drop the stream to cancel
    fn read(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        start: u64,
        end: u64,
        cancel: CancellationToken,
    ) -> Result<ByteStream, MegaError>;
    /// the whole file, MAC verified
    fn download(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        cancel: CancellationToken,
    ) -> Result<ByteStream, MegaError>;
    /// file attribute 0 (120 px square) / 1 (preview); None when the node has none
    async fn attr(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        kind: u32,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<u8>>, MegaError>;
    /// forget cached download URLs (the account session changed)
    fn clear_download_urls(&self) {}
}

#[async_trait]
impl MegaBackend for MegaClient {
    async fn load(&self, link: &MegaLink) -> Result<MegaNode, MegaError> {
        MegaClient::load(self, link).await
    }
    fn read(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        start: u64,
        end: u64,
        cancel: CancellationToken,
    ) -> Result<ByteStream, MegaError> {
        MegaClient::read(self, link, node, start, end, cancel)
    }
    fn download(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        cancel: CancellationToken,
    ) -> Result<ByteStream, MegaError> {
        MegaClient::download(self, link, node, cancel)
    }
    async fn attr(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        kind: u32,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<u8>>, MegaError> {
        MegaClient::attr(self, link, node, kind, cancel).await
    }
    fn clear_download_urls(&self) {
        MegaClient::clear_download_urls(self)
    }
}
