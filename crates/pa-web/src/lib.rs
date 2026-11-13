//! PeekAround in the browser: the same Slint UI as the desktop and Android apps, compiled to
//! WebAssembly and drawn on a canvas. The page comes from the PeekAround server (`pa-cli`, the
//! Docker image), and that server is the only backend it talks to.

#![cfg(target_arch = "wasm32")]

mod host;
mod page;

use std::rc::Rc;

use wasm_bindgen::prelude::*;

/// `log` records go to the browser console.
struct ConsoleLog;

impl log::Log for ConsoleLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = JsValue::from_str(&format!("{}: {}", r.target(), r.args()));
        match r.level() {
            log::Level::Error => web_sys::console::error_1(&line),
            log::Level::Warn => web_sys::console::warn_1(&line),
            _ => web_sys::console::log_1(&line),
        }
    }
    fn flush(&self) {}
}

static LOG: ConsoleLog = ConsoleLog;

/// Called by the page once the module is loaded.
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let _ = log::set_logger(&LOG);
    log::set_max_level(log::LevelFilter::Info);
    let origin = web_sys::window()
        .ok_or("no window")?
        .location()
        .origin()?;
    let client = pa_client::Client::new(&origin).map_err(|e| e.to_string())?;
    let backend = pa_ui::backend::HttpBackend::new(client, pa_ui::rt::Handle);
    pa_ui::run_with(
        backend,
        pa_ui::rt::Handle,
        pa_ui::UiOptions {
            // any path: the browser build keeps the preferences in localStorage
            prefs_path: Some("prefs".into()),
            ..Default::default()
        },
        pa_ui::Integration {
            host: Rc::new(host::WebHost),
            show: true,
            setup: Some(Box::new(|w| {
                page::fit(w);
                page::back_button(w);
            })),
        },
    )
    .map_err(|e| JsValue::from_str(&e.to_string()))
}
