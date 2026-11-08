//! Persistent state of the phone ↔ PC link (1.x `LinkPrefs.kt`, SharedPreferences "peek_link";
//! here `link.json` in the app's files dir, shared by the UI and the ":link" process) and the
//! live status the service writes for the UI (`link-state.json`).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::codec::valid_key;

/// a shared folder
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkFolder {
    /// stable short id (not the path)
    pub key: String,
    /// shown name
    pub name: String,
    /// absolute path on the phone
    pub path: String,
}

/// `link.json`
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LinkPrefs {
    /// sharing switched on
    pub enabled: bool,
    /// server base URL (no trailing slash)
    pub url: Option<String>,
    /// pairing token
    pub token: Option<String>,
    /// the PC's host name (welcome)
    pub server_name: Option<String>,
    /// generated once per install
    pub device_id: Option<String>,
    /// name shown on the PC
    pub device_name: Option<String>,
    /// shared folders
    pub folders: Vec<LinkFolder>,
}

const FILE: &str = "link.json";
const STATE_FILE: &str = "link-state.json";

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, path)
}

impl LinkPrefs {
    /// read `link.json` (defaults when missing)
    pub fn load(files: &Path) -> LinkPrefs {
        let mut p: LinkPrefs = std::fs::read_to_string(files.join(FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        p.folders
            .retain(|f| valid_key(&f.key) && !f.path.is_empty());
        p
    }

    /// write `link.json`
    pub fn save(&self, files: &Path) -> std::io::Result<()> {
        write_atomic(
            &files.join(FILE),
            &serde_json::to_vec_pretty(self).unwrap_or_default(),
        )
    }

    /// the device id, generated (and saved by the caller) on first use
    pub fn ensure_device_id(&mut self) -> String {
        if let Some(id) = self.device_id.as_ref().filter(|s| s.len() >= 8) {
            return id.clone();
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.device_id = Some(id.clone());
        id
    }

    /// New folder list from the UI (name, path): keeps the key of a path that was already shared.
    pub fn assign_keys(&self, input: &[(String, String)]) -> Vec<LinkFolder> {
        let mut used: Vec<String> = vec![];
        let mut out: Vec<LinkFolder> = vec![];
        for (name, path) in input {
            if out.iter().any(|f| f.path == *path) {
                continue;
            }
            let mut key = self
                .folders
                .iter()
                .find(|f| f.path == *path)
                .map(|f| f.key.clone());
            if key.as_ref().is_none_or(|k| used.contains(k)) {
                loop {
                    let k = format!(
                        "k{}",
                        &uuid::Uuid::new_v4().simple().to_string()[..8]
                    );
                    if !used.contains(&k) {
                        key = Some(k);
                        break;
                    }
                }
            }
            let key = key.unwrap();
            used.push(key.clone());
            let name = if name.is_empty() {
                path.rsplit('/').next().unwrap_or(path).to_string()
            } else {
                name.clone()
            };
            out.push(LinkFolder {
                key,
                name,
                path: path.clone(),
            });
        }
        out
    }
}

/// live state (1.x `LinkHub`): "off" | "connecting" | "pending" | "online" | "rejected" | "error"
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LinkStatus {
    /// state
    pub state: String,
    /// pairing code (pending)
    pub code: Option<String>,
    /// last error ("forgotten", "no network", "HTTP 403", …)
    pub error: Option<String>,
    /// requests being served
    pub active: usize,
}

impl Default for LinkStatus {
    fn default() -> Self {
        LinkStatus {
            state: "off".into(),
            code: None,
            error: None,
            active: 0,
        }
    }
}

impl LinkStatus {
    /// read `link-state.json` ("off" when missing)
    pub fn load(files: &Path) -> LinkStatus {
        std::fs::read_to_string(files.join(STATE_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// write `link-state.json`
    pub fn save(&self, files: &Path) {
        let _ = write_atomic(
            &files.join(STATE_FILE),
            &serde_json::to_vec(self).unwrap_or_default(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_survive_a_new_folder_list() {
        let mut p = LinkPrefs::default();
        p.folders = p.assign_keys(&[
            ("Camera".into(), "/sdcard/DCIM/Camera".into()),
            ("".into(), "/sdcard/Pictures".into()),
            ("dup".into(), "/sdcard/Pictures".into()),
        ]);
        assert_eq!(p.folders.len(), 2);
        assert_eq!(p.folders[1].name, "Pictures");
        assert!(p.folders.iter().all(|f| valid_key(&f.key)));
        let cam = p.folders[0].key.clone();
        let next = p.assign_keys(&[
            ("Movies".into(), "/sdcard/Movies".into()),
            ("Camera".into(), "/sdcard/DCIM/Camera".into()),
        ]);
        assert_eq!(next[1].key, cam);
        assert_ne!(next[0].key, cam);
    }

    #[test]
    fn prefs_and_state_files() {
        let d = std::env::temp_dir().join(format!("pa-phone-prefs-{}", std::process::id()));
        let mut p = LinkPrefs::load(&d);
        assert!(!p.enabled);
        let id = p.ensure_device_id();
        assert_eq!(p.ensure_device_id(), id);
        p.enabled = true;
        p.url = Some("http://10.0.0.2:1313".into());
        p.save(&d).unwrap();
        assert_eq!(LinkPrefs::load(&d), p);
        assert_eq!(LinkStatus::load(&d).state, "off");
        LinkStatus {
            state: "pending".into(),
            code: Some("4821".into()),
            ..Default::default()
        }
        .save(&d);
        assert_eq!(LinkStatus::load(&d).code.as_deref(), Some("4821"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
