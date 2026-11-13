//! Facts about files the user handed over (picked, dropped, shared): the file system on the
//! native targets, the registry of `web.rs` in the browser, where a page never sees a path.

use std::path::Path;

/// Is `p` a file that can be read?
pub(crate) fn is_file(p: &Path) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return p.is_file();
    #[cfg(target_arch = "wasm32")]
    return crate::web::file(p).is_some();
}

/// Is `p` a directory? (in the browser: a folder the user picked)
pub(crate) fn is_dir(p: &Path) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return p.is_dir();
    #[cfg(target_arch = "wasm32")]
    return crate::web::is_dir(p);
}

/// The entries of a directory, sorted.
pub(crate) fn read_dir(p: &Path) -> Vec<std::path::PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(p)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    #[cfg(target_arch = "wasm32")]
    let mut v = crate::web::read_dir(p);
    v.sort();
    v
}

/// Size in bytes (0 when unknown).
pub(crate) fn size(p: &Path) -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    return std::fs::metadata(p).map(|m| m.len() as i64).unwrap_or(0);
    #[cfg(target_arch = "wasm32")]
    return crate::web::size(p).unwrap_or(0) as i64;
}

/// Modification time (ms since the epoch).
pub(crate) fn mtime_ms(p: &Path) -> Option<i64> {
    #[cfg(not(target_arch = "wasm32"))]
    return std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);
    #[cfg(target_arch = "wasm32")]
    return crate::web::file(p).map(|f| f.last_modified() as i64);
}
