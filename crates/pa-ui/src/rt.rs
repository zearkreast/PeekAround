//! The executor seam: futures run on the binary's tokio runtime, results come back to the Slint
//! event loop (UI thread) through a runtime-agnostic oneshot polled by `slint::spawn_local`.
//! (The wasm build will swap the tokio half for `wasm_bindgen_futures::spawn_local`.)

use std::future::Future;

/// Runtime handle.
#[derive(Clone)]
pub struct Rt {
    handle: tokio::runtime::Handle,
}

impl Rt {
    /// wrap a tokio runtime handle
    pub fn new(handle: tokio::runtime::Handle) -> Rt {
        Rt { handle }
    }

    /// the tokio handle
    pub fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    /// Run `fut` in the background, then `then(result)` on the UI thread.
    pub fn spawn_then<T: Send + 'static>(
        &self,
        fut: impl Future<Output = T> + Send + 'static,
        then: impl FnOnce(T) + 'static,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.handle.spawn(async move {
            let _ = tx.send(fut.await);
        });
        let _ = slint::spawn_local(async move {
            if let Ok(v) = rx.await {
                then(v);
            }
        });
    }
}
