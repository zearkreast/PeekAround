//! PeekAround for Android (the 1.x Capacitor app, packages/android, rewritten on Slint):
//!
//! - UI process: `android_main` (NativeActivity, `app.peekaround.MainActivity`) runs the first-run
//!   screen ([`setup`]) and then pa-ui in one of two modes ([`config::Mode`]): **local** — the
//!   PeekAround server in-process over the phone's folders (needs "All files access", like 1.x) —
//!   or **server** — a PC / Docker PeekAround over HTTP/WS.
//! - ":link" process: `PeekService` (a foreground service with a Stop action) runs the phone side of
//!   the phone ↔ PC folder sharing ([`pa_phone::LinkClient`]) through the JNI entry points in
//!   [`service`], so sharing goes on with the screen off and after the UI process is gone.
//!
//! Android APIs (permissions, intents, clipboard, notifications, video frames) are Java methods of
//! MainActivity / PeekService called through JNI ([`platform`]); off Android they are stubs so the
//! crate builds and its logic can be checked on the desktop.

pub mod config;
pub mod host;
pub mod platform;
pub mod probe;
pub mod setup;
#[cfg(target_os = "android")]
mod service;

use std::path::Path;
#[cfg(target_os = "android")]
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use pa_ui::backend::{Backend, HttpBackend};

use config::Mode;

/// the app version (= workspace version; versionName of the APK)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: slint::android::AndroidApp) {
    platform::init_logging();
    let files = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/data/app.peekaround/files"));
    if let Err(e) = slint::android::init(app) {
        log::error!("slint init: {e}");
        return;
    }
    run(&files);
    // the UI process ends with the activity; sharing lives on in the ":link" process
    platform::finish();
}

/// The UI process: first-run screen when no mode is saved, then pa-ui.
pub fn run(files: &Path) {
    log::info!("start {VERSION} ({})", std::env::consts::ARCH);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("pa-worker")
        .enable_all()
        .build()
        .expect("tokio runtime");
    let _ = std::fs::create_dir_all(files);
    let ffmpeg = bundled_ffmpeg();
    match &ffmpeg {
        Some(p) => pa_ui::video::set_ffmpeg_path(p.clone()),
        None => log::warn!("no ffmpeg in this build: videos do not play"),
    }
    let mode = match config::load_mode(files) {
        Some(m) => m,
        None => match setup::run(files, runtime.handle().clone()) {
            Some(m) => m,
            None => return,
        },
    };
    let mut server: Option<Arc<pa_server::Server>> = None;
    let backend: Arc<dyn Backend> = match &mode {
        Mode::Local => match runtime.block_on(start_local_server(files, ffmpeg.as_deref())) {
            Ok(s) => {
                let s = Arc::new(s);
                let c = pa_client::Client::new(&s.url).expect("local url");
                server = Some(s);
                HttpBackend::new(c, runtime.handle().clone())
            }
            Err(e) => {
                log::error!("local server: {e}");
                // back to the first-run screen next time instead of a dead app
                config::clear_mode(files);
                platform::toast(&format!("PeekAround: {e}"));
                return;
            }
        },
        Mode::Server { url, .. } => match pa_client::Client::new(url) {
            Ok(c) => {
                // a phone paired with that PC (folder sharing) sees what the PC shows paired phones
                let link = pa_phone::LinkPrefs::load(files);
                if let (Some(tok), Some(id), Some(lurl)) = (&link.token, &link.device_id, &link.url)
                {
                    if config::same_server(lurl, url) {
                        c.set_device_auth(Some(pa_client::DeviceAuth {
                            device_id: id.clone(),
                            token: tok.clone(),
                        }));
                    }
                }
                HttpBackend::new(c, runtime.handle().clone())
            }
            Err(e) => {
                log::error!("{url}: {e}");
                config::clear_mode(files);
                return;
            }
        },
    };

    // sharing that was on comes back with the UI (1.x: resumed from the visible app only)
    if let Mode::Server { url, .. } = &mode {
        let link = pa_phone::LinkPrefs::load(files);
        if link.enabled && link.url.as_deref().is_some_and(|u| config::same_server(u, url)) {
            platform::start_link_service();
        }
    }
    let host: Rc<dyn pa_ui::host::Host> =
        Rc::new(host::AndroidHost::new(files.to_path_buf(), mode.clone()));
    let opts = pa_ui::UiOptions {
        prefs_path: Some(files.join("ui-prefs.json")),
        #[cfg(target_os = "android")]
        script: None,
        #[cfg(not(target_os = "android"))]
        script: std::env::var("PA_SCRIPT").ok().filter(|s| !s.trim().is_empty()),
        autoscroll: None,
        frame_log: false,
    };
    let integ = pa_ui::Integration {
        host,
        show: true,
        setup: None,
    };
    if let Err(e) = pa_ui::run_with(backend, runtime.handle().clone(), opts, integ) {
        log::error!("ui: {e}");
    }
    if let Some(s) = server {
        runtime.block_on(async move {
            let _ = tokio::time::timeout(Duration::from_secs(3), s.close()).await;
        });
    }
    runtime.shutdown_timeout(Duration::from_millis(500));
}

/// The phone's media folders as libraries on the first start (1.x `setupDefaults`: Camera,
/// Pictures, Movies, Download, WhatsApp; else DCIM).
pub fn default_roots(storage: &Path) -> Vec<pa_server::RootSpec> {
    let want: [(&str, &str); 5] = [
        ("DCIM/Camera", "Kamera"),
        ("Pictures", "Pictures"),
        ("Movies", "Movies"),
        ("Download", "Download"),
        ("Android/media/com.whatsapp/WhatsApp/Media", "WhatsApp"),
    ];
    let mut out: Vec<pa_server::RootSpec> = want
        .iter()
        .map(|(rel, name)| (storage.join(rel), *name))
        .filter(|(p, _)| p.is_dir())
        .map(|(p, name)| pa_server::RootSpec {
            path: p.to_string_lossy().into_owned(),
            name: Some(name.to_string()),
            split: false,
        })
        .collect();
    if out.is_empty() {
        let dcim = storage.join("DCIM");
        if dcim.is_dir() {
            out.push(pa_server::RootSpec {
                path: dcim.to_string_lossy().into_owned(),
                name: Some("DCIM".into()),
                split: false,
            });
        }
    }
    out
}

/// ffmpeg ships in the APK as `libffmpeg.so` (Android only runs executables from the native
/// library folder). It has decoders only: no ffprobe (the server reads `ffmpeg -i` instead) and no
/// H.264 encoder, so video exports stay unavailable on the phone.
fn bundled_ffmpeg() -> Option<std::path::PathBuf> {
    let p = platform::native_lib_dir()?.join("libffmpeg.so");
    p.is_file().then_some(p)
}

/// Local mode: the server of the desktop app, on 127.0.0.1 only, data in the app's private files.
async fn start_local_server(
    files: &Path,
    ffmpeg: Option<&Path>,
) -> Result<pa_server::Server, pa_server::StartError> {
    let data = files.join("data");
    let first = !data.join("peekaround.db").exists();
    let mut o = pa_server::ServerOptions::new(data);
    o.mode = pa_core::RunMode::Desktop;
    o.host = "127.0.0.1".into();
    o.port = 0;
    if first {
        o.roots = default_roots(&platform::storage_root());
    }
    o.mdns = false;
    o.version = VERSION.to_string();
    o.ffmpeg_path = Some(match ffmpeg {
        Some(p) => p.to_string_lossy().into_owned(),
        None => "/system/bin/peekaround-no-ffmpeg".into(),
    });
    o.ffprobe_path = Some("/system/bin/peekaround-no-ffprobe".into());
    log::info!("local server: {} roots", o.roots.len());
    pa_server::Server::start(o).await
}
