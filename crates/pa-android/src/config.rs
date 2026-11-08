//! How the app is connected (1.x `backend/android/config.ts` `saveMode`): `mode.json` in the
//! app's private files dir. Missing → the first-run screen.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// local phone gallery or a PeekAround server
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Mode {
    /// the phone's own folders (server in-process)
    Local,
    /// a PC / Docker server
    Server {
        /// base URL, e.g. `http://192.168.1.20:1313`
        url: String,
        /// its host name (shown in Settings)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

const FILE: &str = "mode.json";

/// the saved mode
pub fn load_mode(files: &Path) -> Option<Mode> {
    let s = std::fs::read_to_string(files.join(FILE)).ok()?;
    serde_json::from_str(&s).ok()
}

/// save the mode
pub fn save_mode(files: &Path, m: &Mode) -> std::io::Result<()> {
    std::fs::create_dir_all(files)?;
    let tmp = files.join("mode.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(m).unwrap_or_default())?;
    std::fs::rename(tmp, files.join(FILE))
}

/// "Change connection": the next start shows the first-run screen
pub fn clear_mode(files: &Path) {
    let _ = std::fs::remove_file(files.join(FILE));
}

/// scheme + host + port of two base URLs match
pub fn same_server(a: &str, b: &str) -> bool {
    match (url::Url::parse(a), url::Url::parse(b)) {
        (Ok(a), Ok(b)) => {
            a.scheme() == b.scheme()
                && a.host_str() == b.host_str()
                && a.port_or_known_default() == b.port_or_known_default()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_json_round_trip() {
        let d = std::env::temp_dir().join(format!("pa-android-mode-{}", std::process::id()));
        assert_eq!(load_mode(&d), None);
        let m = Mode::Server {
            url: "http://192.168.1.20:1313".into(),
            name: Some("pc".into()),
        };
        save_mode(&d, &m).unwrap();
        assert_eq!(load_mode(&d), Some(m));
        let raw = std::fs::read_to_string(d.join(FILE)).unwrap();
        assert!(raw.contains("\"mode\":\"server\""), "{raw}");
        save_mode(&d, &Mode::Local).unwrap();
        assert_eq!(load_mode(&d), Some(Mode::Local));
        clear_mode(&d);
        assert_eq!(load_mode(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn same_server_compares_origin() {
        assert!(same_server("http://10.0.0.2:1313", "http://10.0.0.2:1313/"));
        assert!(!same_server("http://10.0.0.2:1313", "http://10.0.0.2:8080"));
        assert!(same_server("http://pc.local", "http://pc.local:80"));
    }
}
