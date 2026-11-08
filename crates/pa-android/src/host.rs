//! pa-ui's [`Host`] on Android: links, clipboard, and the Settings cards of the 1.x app
//! (Connection: how the app is connected / change it; phone folder sharing in server mode).

use std::path::PathBuf;

use pa_ui::host::Host;

use crate::config::Mode;
use crate::platform;

/// the Android shell
pub struct AndroidHost {
    pub(crate) files: PathBuf,
    pub(crate) mode: Mode,
    pub(crate) rt: tokio::runtime::Handle,
}

impl AndroidHost {
    /// `files`: the app's private files dir
    pub fn new(files: PathBuf, mode: Mode, rt: tokio::runtime::Handle) -> AndroidHost {
        AndroidHost { files, mode, rt }
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
}
