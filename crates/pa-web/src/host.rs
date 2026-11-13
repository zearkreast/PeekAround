//! What the page can do for the UI in place of a desktop shell: links, the clipboard, fullscreen,
//! the file picker of the browser.

use std::path::PathBuf;

use pa_ui::host::{FileFilter, Host, HostFut};

/// The browser tab.
pub struct WebHost;

impl Host for WebHost {
    fn platform(&self) -> &'static str {
        "web"
    }

    fn phone(&self) -> bool {
        pa_ui::web::coarse_pointer()
    }

    fn pick_files(&self, _title: &str, filters: Vec<FileFilter>) -> HostFut<Vec<PathBuf>> {
        let accept = filters
            .iter()
            .flat_map(|f| f.extensions.iter().map(|e| format!(".{e}")))
            .collect::<Vec<_>>()
            .join(",");
        Box::pin(async move { pa_ui::web::pick_files(true, &accept, false).await })
    }

    fn pick_folders(&self, _title: &str) -> HostFut<Vec<PathBuf>> {
        Box::pin(pa_ui::web::pick_files(true, "", true))
    }

    fn open_url(&self, url: &str) {
        if let Some(w) = web_sys::window() {
            let _ = w.open_with_url_and_target(url, "_blank");
        }
    }

    fn copy_text(&self, text: &str) {
        if let Some(w) = web_sys::window() {
            let _ = w.navigator().clipboard().write_text(text);
        }
    }

    fn toggle_fullscreen(&self) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if doc.fullscreen_element().is_some() {
            doc.exit_fullscreen();
        } else if let Some(root) = doc.document_element() {
            let _ = root.request_fullscreen();
        }
    }
}
