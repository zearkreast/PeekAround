//! Where the app keeps its files — the same places as the Electron app, so an upgrade keeps the
//! libraries, favorites, albums, settings and paired phones:
//!
//! | | Electron (`app.getPath`) | here |
//! |---|---|---|
//! | userData | `%APPDATA%\PeekAround` · `~/.config/PeekAround` · `~/Library/Application Support/PeekAround` | [`Paths::user_data`] |
//! | server data | `<userData>/data` (`peekaround.db`, thumbnails, caches) | [`Paths::data`] |
//! | shell prefs | `<userData>/desktop.json` | [`Paths::desktop_json`] |
//! | logs | `<userData>/logs` (Windows / Linux) · `~/Library/Logs/PeekAround` | [`Paths::logs`] |
//!
//! Not in the Electron app: `<userData>/ui-prefs.json` (the per-device UI preferences the web UI kept in localStorage).

use std::path::PathBuf;

/// app name = Electron's `productName`
pub const APP: &str = "PeekAround";

/// Resolved directories.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Electron's userData
    pub user_data: PathBuf,
    pub data: PathBuf,
    pub logs: PathBuf,
}

impl Paths {
    /// OS defaults, or everything under `root` (`--user-data DIR`, tests)
    pub fn detect(root: Option<PathBuf>) -> Paths {
        let user_data = root
            .or_else(|| std::env::var_os("PEEK_USER_DATA").map(PathBuf::from))
            .unwrap_or_else(|| {
                // Windows: Roaming AppData; Linux: $XDG_CONFIG_HOME or ~/.config; macOS: Application Support
                dirs::config_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join(APP)
            });
        let logs = if cfg!(target_os = "macos") {
            dirs::home_dir()
                .map(|h| h.join("Library/Logs").join(APP))
                .unwrap_or_else(|| user_data.join("logs"))
        } else {
            user_data.join("logs")
        };
        Paths {
            data: user_data.join("data"),
            logs,
            user_data,
        }
    }

    /// shell preferences (Electron format)
    pub fn desktop_json(&self) -> PathBuf {
        self.user_data.join("desktop.json")
    }

    pub fn ui_prefs(&self) -> PathBuf {
        self.user_data.join("ui-prefs.json")
    }

    /// the database exists already (not the first start)
    pub fn has_db(&self) -> bool {
        self.data.join("peekaround.db").exists()
    }
}

/// First-run library roots: the OS Pictures and Videos folders (Electron: app.getPath('pictures'|'videos')).
pub fn first_run_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in [dirs::picture_dir(), dirs::video_dir()]
        .into_iter()
        .flatten()
    {
        if d.is_dir() && !out.contains(&d) {
            out.push(d);
        }
    }
    out
}
