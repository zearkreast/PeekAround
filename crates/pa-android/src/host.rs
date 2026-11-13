//! pa-ui's [`Host`] on Android: links, clipboard, and the Settings cards of the 1.x app
//! (Connection: how the app is connected / change it; phone folder sharing in server mode).

use std::path::{Path, PathBuf};

use pa_phone::{LinkPrefs, LinkStatus};
use pa_ui::host::{AndroidState, Host, Shared};

use crate::config::{self, Mode};
use crate::platform;

/// the Android shell
pub struct AndroidHost {
    files: PathBuf,
    mode: Mode,
}

impl AndroidHost {
    /// `files`: the app's private files dir
    pub fn new(files: PathBuf, mode: Mode) -> AndroidHost {
        AndroidHost { files, mode }
    }

    fn server(&self) -> Option<&str> {
        match &self.mode {
            Mode::Server { url, .. } => Some(url),
            Mode::Local => None,
        }
    }
}

/// the JSON of `MainActivity.takeShared`
pub fn parse_shared(json: &str) -> Option<Shared> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let files: Vec<PathBuf> = v["files"]
        .as_array()
        .map(|a| a.iter().filter_map(|f| f.as_str()).map(PathBuf::from).collect())
        .unwrap_or_default();
    let text = v["text"].as_str().map(str::to_string).filter(|t| !t.trim().is_empty());
    (!files.is_empty() || text.is_some()).then_some(Shared { files, text })
}

/// folders offered for sharing (the usual choices of the 1.x folder picker), shared ones first
pub fn share_candidates(storage: &Path, prefs: &LinkPrefs) -> Vec<(String, String, bool)> {
    let mut out: Vec<(String, String, bool)> = prefs
        .folders
        .iter()
        .map(|f| (f.name.clone(), f.path.clone(), true))
        .collect();
    for rel in [
        "DCIM/Camera",
        "DCIM/Screenshots",
        "Pictures",
        "Pictures/Screenshots",
        "Movies",
        "Download",
        "Android/media/com.whatsapp/WhatsApp/Media/WhatsApp Images",
        "Android/media/com.whatsapp/WhatsApp/Media/WhatsApp Video",
    ] {
        let p = storage.join(rel);
        let path = p.to_string_lossy().into_owned();
        if p.is_dir() && !out.iter().any(|f| f.1 == path) {
            let name = rel.rsplit('/').next().unwrap_or(rel).to_string();
            out.push((name, path, false));
        }
    }
    out
}

/// what the sharing service has to do after a Settings action
#[derive(Debug, PartialEq, Eq)]
pub enum ServiceCmd {
    /// nothing
    None,
    /// start (or keep) the service
    Start,
    /// stop it
    Stop,
    /// the folders changed
    Folders,
}

/// "share:on|off", "folder:on|off:<path>", "reset" applied to link.json (server mode, `server` =
/// the connected server's URL)
pub fn apply_share_action(
    prefs: &mut LinkPrefs,
    server: &str,
    action: &str,
    device_name: impl FnOnce() -> String,
) -> ServiceCmd {
    match action {
        "share:on" => {
            if prefs.folders.is_empty() {
                return ServiceCmd::None;
            }
            // another PC: its pairing starts over
            if !prefs
                .url
                .as_deref()
                .is_some_and(|u| config::same_server(u, server))
            {
                prefs.token = None;
                prefs.server_name = None;
            }
            prefs.url = Some(server.trim_end_matches('/').to_string());
            prefs.ensure_device_id();
            if prefs
                .device_name
                .as_deref()
                .is_none_or(|n| n.trim().is_empty())
            {
                prefs.device_name = Some(device_name());
            }
            prefs.enabled = true;
            ServiceCmd::Start
        }
        "share:off" => {
            prefs.enabled = false;
            ServiceCmd::Stop
        }
        "reset" => {
            prefs.token = None;
            let was = prefs.enabled;
            prefs.enabled = false;
            if was {
                ServiceCmd::Stop
            } else {
                ServiceCmd::None
            }
        }
        _ => {
            let (on, path) = if let Some(p) = action.strip_prefix("folder:on:") {
                (true, p)
            } else if let Some(p) = action.strip_prefix("folder:off:") {
                (false, p)
            } else {
                return ServiceCmd::None;
            };
            let mut list: Vec<(String, String)> = prefs
                .folders
                .iter()
                .filter(|f| f.path != path)
                .map(|f| (f.name.clone(), f.path.clone()))
                .collect();
            if on {
                let name = path.rsplit('/').next().unwrap_or(path).to_string();
                list.push((name, path.to_string()));
            }
            prefs.folders = prefs.assign_keys(&list);
            if !prefs.enabled {
                ServiceCmd::None
            } else if prefs.folders.is_empty() {
                prefs.enabled = false;
                ServiceCmd::Stop
            } else {
                ServiceCmd::Folders
            }
        }
    }
}

impl Host for AndroidHost {
    fn platform(&self) -> &'static str {
        "android"
    }
    fn open_url(&self, url: &str) {
        if url.starts_with("http://") || url.starts_with("https://") {
            platform::open_url(url);
        }
    }
    fn copy_text(&self, text: &str) {
        platform::copy_text(text);
    }
    fn take_shared(&self) -> Option<Shared> {
        parse_shared(&platform::take_shared()?)
    }
    fn download_dir(&self) -> Option<PathBuf> {
        let dir = platform::storage_root().join("Download").join("PeekAround");
        // no storage permission (server mode on Android 10 and older): the app's own folder
        if std::fs::create_dir_all(&dir).is_ok() {
            Some(dir)
        } else {
            Some(self.files.join("downloads"))
        }
    }
    fn file_saved(&self, path: &Path) {
        platform::scan_file(&path.to_string_lossy());
    }
    fn share_dir(&self) -> Option<PathBuf> {
        // Context.getCacheDir(), where ShareProvider serves from
        Some(self.files.parent()?.join("cache").join("share"))
    }
    fn share_files(&self, paths: Vec<PathBuf>) {
        let list: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        if let Ok(json) = serde_json::to_string(&list) {
            platform::share_files(&json);
        }
    }
    fn android(&self) -> Option<AndroidState> {
        let prefs = LinkPrefs::load(&self.files);
        let st = LinkStatus::load(&self.files);
        let pc = prefs
            .server_name
            .clone()
            .or_else(|| {
                self.server()
                    .map(|u| u.split("://").nth(1).unwrap_or(u).to_string())
            })
            .unwrap_or_default();
        Some(AndroidState {
            server: self.server().map(str::to_string),
            share_on: prefs.enabled,
            // a stale file of a stopped service must not show "online"
            share_state: if prefs.enabled || st.state == "rejected" || st.state == "error" {
                st.state
            } else {
                "off".into()
            },
            share_code: st.code,
            share_error: st.error,
            share_active: st.active,
            pc_name: pc,
            folders: share_candidates(&platform::storage_root(), &prefs),
        })
    }
    fn android_action(&self, action: &str) {
        if action == "change" {
            config::clear_mode(&self.files);
            platform::restart();
            return;
        }
        let Some(server) = self.server() else { return };
        let mut prefs = LinkPrefs::load(&self.files);
        let cmd = apply_share_action(&mut prefs, server, action, platform::device_name);
        if let Err(e) = prefs.save(&self.files) {
            log::warn!("link.json: {e}");
        }
        match cmd {
            ServiceCmd::Start => {
                platform::request_notifications();
                if !platform::start_link_service() {
                    log::warn!("the sharing service could not start");
                }
            }
            ServiceCmd::Stop => platform::stop_link_service(),
            ServiceCmd::Folders => platform::link_folders_changed(),
            ServiceCmd::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_json() {
        let s = parse_shared(r#"{"files":["/c/incoming/1/a.jpg"],"text":"https://mega.nz/x"}"#).unwrap();
        assert_eq!(s.files, vec![PathBuf::from("/c/incoming/1/a.jpg")]);
        assert_eq!(s.text.as_deref(), Some("https://mega.nz/x"));
        assert_eq!(parse_shared(r#"{"files":[]}"#), None);
        assert_eq!(parse_shared(""), None);
    }

    #[test]
    fn share_actions_edit_link_json() {
        let mut p = LinkPrefs::default();
        let srv = "http://10.0.0.2:1313";
        let name = || "Pixel".to_string();
        assert_eq!(apply_share_action(&mut p, srv, "share:on", name), ServiceCmd::None);
        assert_eq!(
            apply_share_action(&mut p, srv, "folder:on:/s/DCIM/Camera", name),
            ServiceCmd::None
        );
        assert_eq!(p.folders[0].name, "Camera");
        assert_eq!(apply_share_action(&mut p, srv, "share:on", name), ServiceCmd::Start);
        assert!(p.enabled && p.device_id.is_some());
        assert_eq!(p.device_name.as_deref(), Some("Pixel"));
        assert_eq!(p.url.as_deref(), Some(srv));
        p.token = Some("t".into());
        assert_eq!(
            apply_share_action(&mut p, srv, "folder:on:/s/Pictures", name),
            ServiceCmd::Folders
        );
        let cam_key = p.folders[0].key.clone();
        assert_eq!(
            apply_share_action(&mut p, srv, "folder:off:/s/Pictures", name),
            ServiceCmd::Folders
        );
        assert_eq!(p.folders[0].key, cam_key);
        // the same PC keeps the token, another one drops it
        apply_share_action(&mut p, srv, "share:on", name);
        assert_eq!(p.token.as_deref(), Some("t"));
        apply_share_action(&mut p, "http://10.0.0.9:1313", "share:on", name);
        assert_eq!(p.token, None);
        assert_eq!(
            apply_share_action(&mut p, srv, "folder:off:/s/DCIM/Camera", name),
            ServiceCmd::Stop
        );
        assert!(!p.enabled);
        p.token = Some("t".into());
        assert_eq!(apply_share_action(&mut p, srv, "reset", name), ServiceCmd::None);
        assert_eq!(p.token, None);
    }
}
