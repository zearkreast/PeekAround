//! `desktop.json` — the Electron shell's preferences file (shell.ts `DesktopPrefs`), read and
//! written in the same format so both apps understand it (keys the Rust app does not know are
//! kept as they are).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// LAN share
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lan {
    pub enabled: bool,
    /// port (default 1313)
    pub port: u16,
}

impl Default for Lan {
    fn default() -> Self {
        Lan {
            enabled: false,
            port: 1313,
        }
    }
}

/// window bounds (logical px of the normal = not maximized window)
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximized: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullscreen: Option<bool>,
}

/// shell.ts `DesktopPrefs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPrefs {
    #[serde(default)]
    pub lan: Lan,
    /// fixed local port (the UI origin of the Electron app; kept for the API)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Bounds>,
    /// window zoom (Ctrl + / −)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    #[serde(default)]
    pub start_at_login: bool,
    /// login launch: tray only
    #[serde(default)]
    pub start_hidden: bool,
    /// "auto" | "tray" | "quit"
    #[serde(default = "auto")]
    pub close_action: String,
    /// GPU rendering (read at start)
    #[serde(default = "yes")]
    pub gpu: bool,
    #[serde(default = "yes")]
    pub notify: bool,
    /// daily release check
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// ms epoch of the last successful release check
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update_check: Option<i64>,
    /// newest version the user was told about
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_version: Option<String>,
    /// tray / notification language
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// last release check result
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<pa_ui::host::UpdateInfo>,
    /// the "still running in the tray" notice was shown
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tray_hint_shown: Option<bool>,
    /// keys of other versions
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn auto() -> String {
    "auto".into()
}
fn yes() -> bool {
    true
}

impl Default for DesktopPrefs {
    fn default() -> Self {
        serde_json::from_str("{}").expect("defaults")
    }
}

impl DesktopPrefs {
    /// read (defaults for a missing / broken file)
    pub fn load(path: &Path) -> DesktopPrefs {
        let mut p = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<DesktopPrefs>(&b).ok())
            .unwrap_or_default();
        if !matches!(p.close_action.as_str(), "auto" | "tray" | "quit") {
            p.close_action = auto();
        }
        if p.lan.port == 0 {
            p.lan.port = 1313;
        }
        p
    }

    /// write atomically (tmp + rename); errors are logged (read-only profile: in-memory prefs)
    pub fn save(&self, path: &Path) {
        let write = || -> std::io::Result<()> {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            let tmp = PathBuf::from(format!("{}.tmp", path.display()));
            std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
            std::fs::rename(&tmp, path)
        };
        if let Err(e) = write() {
            log::warn!("saving {}: {e}", path.display());
        }
    }

    /// tray / notification language: the saved one, else the OS locale
    pub fn lang(&self) -> &'static str {
        match self.lang.as_deref() {
            Some("en") => "en",
            Some("tr") => "tr",
            _ => {
                let loc = std::env::var("LANG")
                    .or_else(|_| std::env::var("LC_ALL"))
                    .unwrap_or_default()
                    .to_lowercase();
                if loc.starts_with("tr") || os_locale_is_tr() {
                    "tr"
                } else {
                    "en"
                }
            }
        }
    }
}

#[cfg(windows)]
fn os_locale_is_tr() -> bool {
    // GetUserDefaultUILanguage: LANGID 0x041f = tr-TR
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    unsafe { GetUserDefaultUILanguage() & 0x3ff == 0x1f }
}

#[cfg(not(windows))]
fn os_locale_is_tr() -> bool {
    false
}

/// two-language strings of the shell (shell.ts `L`)
pub fn l(lang: &str, tr: &str, en: &str) -> String {
    if lang == "en" { en } else { tr }.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_electron_file_and_keeps_unknown_keys() {
        let raw = r#"{"lan":{"enabled":true,"port":8080},"port":47213,"bounds":{"x":10,"y":20,"width":1300,"height":800,"maximized":true,"fullscreen":false},
            "zoom":1.25,"startAtLogin":true,"startHidden":false,"closeAction":"tray","gpu":false,"notify":true,"autoUpdate":true,
            "lastUpdateCheck":1700000000000,"notifiedVersion":"1.3.2","lang":"en","trayHintShown":true,
            "lastUpdate":{"current":"1.3.1","latest":"1.3.2","newer":true,"url":"https://github.com/x","checkedAt":1700000000000},"future":{"a":1}}"#;
        let p: DesktopPrefs = serde_json::from_str(raw).unwrap();
        assert!(p.lan.enabled);
        assert_eq!(p.lan.port, 8080);
        assert_eq!(p.port, Some(47213));
        assert_eq!(p.bounds.as_ref().unwrap().maximized, Some(true));
        assert_eq!(p.close_action, "tray");
        assert!(!p.gpu);
        assert_eq!(p.zoom, Some(1.25));
        assert_eq!(
            p.last_update.as_ref().unwrap().latest.as_deref(),
            Some("1.3.2")
        );
        let back = serde_json::to_value(&p).unwrap();
        assert_eq!(back["future"]["a"], 1);
        assert_eq!(back["closeAction"], "tray");
        assert_eq!(back["lan"]["port"], 8080);
        assert_eq!(back["bounds"]["width"], 1300.0);
    }

    #[test]
    fn defaults() {
        let p = DesktopPrefs::default();
        assert_eq!(
            p.lan,
            Lan {
                enabled: false,
                port: 1313
            }
        );
        assert_eq!(p.close_action, "auto");
        assert!(p.gpu && p.notify && p.auto_update);
        assert!(!p.start_at_login && !p.start_hidden);
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("pa-desk-prefs-{}", std::process::id()));
        let f = dir.join("desktop.json");
        let p = DesktopPrefs {
            close_action: "quit".into(),
            lan: Lan {
                enabled: true,
                port: 1313,
            },
            ..Default::default()
        };
        p.save(&f);
        let q = DesktopPrefs::load(&f);
        assert_eq!(q.close_action, "quit");
        assert!(q.lan.enabled);
        let _ = std::fs::remove_dir_all(dir);
    }
}
