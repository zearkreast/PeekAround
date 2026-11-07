//! The embedded PeekAround server (main.ts `startServer`): same options as the Electron app —
//! desktop mode on 127.0.0.1, a fixed port (the saved one, 47213, then 47214–47216, else any),
//! data under userData/data, the OS Pictures + Videos folders as libraries on the first start,
//! the bundled ffmpeg, mDNS, deletes into the OS recycle bin.

use std::path::PathBuf;
use std::sync::Arc;

use pa_server::{DesktopHooks, RootSpec, Server, ServerOptions};

use crate::paths::{first_run_roots, Paths};

/// Deletes go to the OS recycle bin / trash (Electron `shell.trashItem`).
struct Trash;

impl DesktopHooks for Trash {
    fn trash(&self, abs: &std::path::Path) -> Option<std::io::Result<()>> {
        Some(trash::delete(abs).map_err(|e| std::io::Error::other(e.to_string())))
    }
    fn has_trash(&self) -> bool {
        true
    }
}

async fn port_free(p: u16) -> bool {
    tokio::net::TcpListener::bind(("127.0.0.1", p))
        .await
        .is_ok()
}

/// the saved port, then 47213…47216; 0 = any free port
pub async fn pick_port(saved: Option<u16>) -> u16 {
    let mut cands: Vec<u16> = saved.into_iter().filter(|p| *p != 0).collect();
    for p in [47213, 47214, 47215, 47216] {
        if !cands.contains(&p) {
            cands.push(p);
        }
    }
    for p in cands {
        if port_free(p).await {
            return p;
        }
    }
    0
}

/// ffmpeg next to the executable (the packaged app ships it), else $PEEK_FFMPEG / PATH
pub fn ffmpeg_path() -> String {
    if let Ok(p) = std::env::var("PEEK_FFMPEG") {
        if !p.is_empty() {
            return p;
        }
    }
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
    {
        for c in [d.join(name), d.join("bin").join(name)] {
            if c.is_file() {
                return c.to_string_lossy().into_owned();
            }
        }
    }
    "ffmpeg".into()
}

pub async fn start(
    paths: &Paths,
    saved_port: Option<u16>,
    version: &str,
) -> Result<Server, pa_server::StartError> {
    let roots: Vec<RootSpec> = if paths.has_db() {
        vec![]
    } else {
        first_run_roots()
            .into_iter()
            .map(|p| RootSpec {
                path: p.to_string_lossy().into_owned(),
                name: None,
                split: false,
            })
            .collect()
    };
    let mut o = ServerOptions::new(paths.data.clone());
    o.mode = pa_core::RunMode::Desktop;
    o.host = "127.0.0.1".into();
    o.port = pick_port(saved_port).await;
    o.roots = roots;
    o.ffmpeg_path = Some(ffmpeg_path());
    o.ffprobe_path = Some(std::env::var("PEEK_FFPROBE").unwrap_or_else(|_| "ffprobe".into()));
    o.mdns = std::env::var("PEEK_NO_MDNS").is_err();
    o.hooks = Some(Arc::new(Trash));
    o.version = version.to_string();
    log::info!(
        "server starting: data {} port {} roots {}",
        paths.data.display(),
        o.port,
        o.roots.len()
    );
    Server::start(o).await
}
