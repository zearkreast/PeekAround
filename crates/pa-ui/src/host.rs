//! What the UI needs from the program around it (the desktop shell; Electron's preload bridge):
//! native file dialogs, opening links / folders, the LAN share, shell options (autostart, tray,
//! GPU, notifications, updates), window zoom / fullscreen, taskbar progress. `pa-desktop`
//! implements it; [`NoHost`] is the default (remote server / demo / Android client mode).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// Boxed host future (runs on the tokio runtime; results come back to the UI thread).
pub type HostFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// `peek:get-lan` / `peek:set-lan`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanState {
    /// sharing on
    pub enabled: bool,
    /// configured port (default 1313)
    pub port: u16,
    /// reachable addresses while sharing
    pub urls: Vec<String>,
    /// set when switching failed
    pub error: Option<String>,
}

/// Result of a GitHub release check (`UpdateInfo` of shell.ts).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// running version
    pub current: String,
    /// newest release
    pub latest: Option<String>,
    /// latest > current
    pub newer: bool,
    /// release page
    pub url: Option<String>,
    /// ms epoch
    pub checked_at: i64,
    /// failure
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Settings → Desktop (`shellState()` of main.ts).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellState {
    /// open at login
    pub start_at_login: bool,
    /// login launch: tray only
    pub start_hidden: bool,
    /// "auto" | "tray" | "quit"
    pub close_action: String,
    /// GPU rendering (applied at the next start)
    pub gpu: bool,
    /// desktop notifications
    pub notify: bool,
    /// daily release check
    pub auto_update: bool,
    /// autostart can be configured on this platform / build
    pub login_supported: bool,
    /// the renderer this process started with
    pub gpu_active: bool,
    /// app version
    pub version: String,
    /// last release check
    pub last_update: Option<UpdateInfo>,
}

/// A change of [`ShellState`] (only the given fields).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellPatch {
    /// open at login
    pub start_at_login: Option<bool>,
    /// login launch: tray only
    pub start_hidden: Option<bool>,
    /// "auto" | "tray" | "quit"
    pub close_action: Option<String>,
    /// GPU rendering
    pub gpu: Option<bool>,
    /// notifications
    pub notify: Option<bool>,
    /// daily release check
    pub auto_update: Option<bool>,
}

/// File dialog filter.
#[derive(Debug, Clone)]
pub struct FileFilter {
    /// shown name
    pub name: String,
    /// extensions without dot
    pub extensions: Vec<String>,
}

/// Settings → Connection and Phone sharing of the Android app (1.x `AndroidConnectionCard`,
/// `PhoneShareCard`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AndroidState {
    /// None = local mode (the phone's own folders), else the server URL
    pub server: Option<String>,
    /// phone folder sharing is switched on
    pub share_on: bool,
    /// "off" | "connecting" | "pending" | "online" | "rejected" | "error"
    pub share_state: String,
    /// pairing code while pending
    pub share_code: Option<String>,
    /// last error ("forgotten", "HTTP 403", …)
    pub share_error: Option<String>,
    /// requests being served
    pub share_active: usize,
    /// the PC's host name
    pub pc_name: String,
    /// folders that can be shared: (name, path, shared)
    pub folders: Vec<(String, String, bool)>,
}

/// What another app handed to this one (Android "Share").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shared {
    /// local copies of the shared files
    pub files: Vec<PathBuf>,
    /// shared text (a link)
    pub text: Option<String>,
}

/// The desktop shell. Every method has a "not available" default.
pub trait Host {
    /// "win32" | "linux" | "darwin" | "android" | "" (no desktop shell)
    fn platform(&self) -> &'static str {
        ""
    }
    /// a desktop shell is there (Settings → Desktop, LAN share card, native folder picker)
    fn is_desktop(&self) -> bool {
        false
    }
    /// native folder picker (multi selection)
    fn pick_folders(&self, _title: &str) -> HostFut<Vec<PathBuf>> {
        Box::pin(async { vec![] })
    }
    /// native file picker (multi selection)
    fn pick_files(&self, _title: &str, _filters: Vec<FileFilter>) -> HostFut<Vec<PathBuf>> {
        Box::pin(async { vec![] })
    }
    /// native "save as"
    fn save_file(&self, _title: &str, _name: &str) -> HostFut<Option<PathBuf>> {
        Box::pin(async { None })
    }
    /// open an http(s) link in the browser
    fn open_url(&self, _url: &str) {}
    /// put text on the clipboard
    fn copy_text(&self, _text: &str) {}
    /// reveal a file in the file manager
    fn show_in_folder(&self, _path: &str) {}
    /// open the data / logs folder ("data" | "logs"); Err = message
    fn open_path(&self, _which: &str) -> Result<(), String> {
        Err("not supported".into())
    }
    /// LAN share state
    fn lan(&self) -> Option<LanState> {
        None
    }
    /// switch the LAN share
    fn set_lan(&self, _enabled: bool, _port: u16) -> HostFut<LanState> {
        Box::pin(async {
            LanState {
                error: Some("not supported".into()),
                ..Default::default()
            }
        })
    }
    /// shell options
    fn shell(&self) -> Option<ShellState> {
        None
    }
    /// change shell options
    fn set_shell(&self, _patch: ShellPatch) -> Option<ShellState> {
        None
    }
    /// relaunch the app
    fn restart(&self) {}
    /// check GitHub for a newer release now
    fn check_update(&self) -> HostFut<UpdateInfo> {
        Box::pin(async { UpdateInfo::default() })
    }
    /// a release found by the daily check that was not announced yet (marks it announced)
    fn take_pending_update(&self) -> Option<UpdateInfo> {
        None
    }
    /// UI language changed (tray menu, notifications)
    fn set_lang(&self, _lang: &str) {}
    /// window zoom: +1 / −1 / 0 = reset; returns the new factor
    fn zoom(&self, _dir: i32) -> f32 {
        1.0
    }
    /// F11
    fn toggle_fullscreen(&self) {}
    /// a system notification (only when the window is not focused, and notifications are on)
    fn notify(&self, _title: &str, _body: &str) {}
    /// taskbar progress (None = off)
    fn progress(&self, _value: Option<f32>) {}
    /// Android app state for Settings (None elsewhere)
    fn android(&self) -> Option<AndroidState> {
        None
    }
    /// Android Settings actions: "change" (connection), "share:on" / "share:off",
    /// "folder:on:<path>" / "folder:off:<path>", "reset" (pairing)
    fn android_action(&self, _action: &str) {}
    /// files / text shared into the app since the last call
    fn take_shared(&self) -> Option<Shared> {
        None
    }
    /// where downloads are saved (None = the user's Downloads folder)
    fn download_dir(&self) -> Option<PathBuf> {
        None
    }
    /// a file was saved for the user (Android: makes it show up in the gallery apps)
    fn file_saved(&self, _path: &Path) {}
    /// Some = files can be handed to other apps: they have to be put into this folder first
    fn share_dir(&self) -> Option<PathBuf> {
        None
    }
    /// open the system share sheet for files inside [`Host::share_dir`]
    fn share_files(&self, _paths: Vec<PathBuf>) {}
}

/// No desktop shell.
pub struct NoHost;
impl Host for NoHost {}
