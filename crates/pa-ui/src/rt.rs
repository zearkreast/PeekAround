//! The executor seam: futures run on the binary's tokio runtime, results come back to the Slint
//! event loop (UI thread) through a runtime-agnostic oneshot polled by `slint::spawn_local`.
//! In the browser there is one thread and no tokio runtime: futures run on the page's event loop
//! and "blocking" work runs in place.

use std::future::Future;

#[cfg(not(target_arch = "wasm32"))]
pub use tokio::runtime::Handle;
#[cfg(not(target_arch = "wasm32"))]
pub use tokio::task::spawn_blocking;

/// The page's event loop in place of a tokio runtime.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, Default)]
pub struct Handle;

/// `spawn_blocking` never fails in the browser; the type keeps the call sites the same.
#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
pub struct JoinError;

#[cfg(target_arch = "wasm32")]
impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("task failed")
    }
}

/// Run `f` right here: the browser build has no worker threads.
#[cfg(target_arch = "wasm32")]
pub fn spawn_blocking<T>(f: impl FnOnce() -> T) -> std::future::Ready<Result<T, JoinError>> {
    std::future::ready(Ok(f()))
}

#[cfg(target_arch = "wasm32")]
impl Handle {
    /// Run `fut` on the page's event loop.
    pub fn spawn(&self, fut: impl Future<Output = ()> + 'static) {
        wasm_bindgen_futures::spawn_local(fut);
    }

    /// see [`spawn_blocking`]
    pub fn spawn_blocking<T>(
        &self,
        f: impl FnOnce() -> T,
    ) -> std::future::Ready<Result<T, JoinError>> {
        spawn_blocking(f)
    }
}

/// `Send` on the native targets, nothing in the browser (its futures hold JS values).
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
/// `Send` on the native targets, nothing in the browser (its futures hold JS values).
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// Wait `ms` milliseconds.
pub async fn sleep_ms(ms: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    #[cfg(target_arch = "wasm32")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        slint::Timer::single_shot(std::time::Duration::from_millis(ms), move || {
            let _ = tx.send(());
        });
        let _ = rx.await;
    }
}

/// The bytes of a file the user chose (a local path; in the browser one of `web::register`).
pub async fn read_file(path: std::path::PathBuf) -> Result<Vec<u8>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    return tokio::fs::read(&path).await.map_err(|e| e.to_string());
    #[cfg(target_arch = "wasm32")]
    return crate::web::read(&path).await;
}

/// Runtime handle.
#[derive(Clone)]
pub struct Rt {
    handle: Handle,
}

impl Rt {
    /// wrap a tokio runtime handle
    pub fn new(handle: Handle) -> Rt {
        Rt { handle }
    }

    /// the tokio handle
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Run `fut` in the background, then `then(result)` on the UI thread.
    #[cfg(not(target_arch = "wasm32"))]
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

    /// Run `fut` on the page's event loop, then `then(result)`.
    #[cfg(target_arch = "wasm32")]
    pub fn spawn_then<T: 'static>(
        &self,
        fut: impl Future<Output = T> + 'static,
        then: impl FnOnce(T) + 'static,
    ) {
        let _ = slint::spawn_local(async move {
            then(fut.await);
        });
    }
}
