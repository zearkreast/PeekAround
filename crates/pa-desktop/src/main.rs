//! PeekAround desktop app: the Slint UI with the PeekAround server running in-process (the
//! counterpart of the Electron shell, packages/desktop/src/{main,shell}.ts).
//!
//! ```text
//! peekaround-desktop [--hidden] [--user-data DIR]        the app (embedded server)
//! peekaround-desktop --server URL                        UI only, against a running server
//! peekaround-desktop --demo N                            synthetic library, no server
//! peekaround-desktop --gallery [light]                   component gallery (dev aid)
//! env: PA_SCRIPT="wait 2000; shot a.png; nav settings; …" automation / screenshots (pa-ui perf.rs)
//!      PA_AUTOSCROLL=px_per_tick[,seconds]  PA_FRAME_LOG=1  PEEK_LOG=debug  PEEK_USER_DATA=DIR
//!      SLINT_BACKEND=winit-software          software renderer (also: Settings → Desktop → GPU off)
//! ```

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod desktop;
mod electron_prefs;
mod instance;
mod logfile;
mod os;
mod paths;
mod prefs;
mod server;
mod tray;
mod update;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pa_ui::backend::{Backend, DemoBackend, HttpBackend};
use pa_ui::perf::AutoScroll;
use pa_ui::UiOptions;
use slint::ComponentHandle;

use desktop::{DesktopHost, Shared};

/// the app version (= workspace version)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// relaunch after the event loop ends (Settings → Desktop → Restart)
pub static RESTART: AtomicBool = AtomicBool::new(false);

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// ms since the epoch
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

enum Source {
    Embedded,
    Remote(String),
    Demo(usize),
}

fn usage() -> ! {
    eprintln!("usage: peekaround-desktop [--hidden] [--user-data DIR] [--server URL] [--demo N] [--prefs FILE] [--gallery [light]]");
    std::process::exit(2)
}

fn fatal(msg: &str) -> ! {
    log::error!("{msg}");
    let _ = rfd::MessageDialog::new()
        .set_title("PeekAround")
        .set_description(msg)
        .set_level(rfd::MessageLevel::Error)
        .show();
    std::process::exit(1)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut source = Source::Embedded;
    let mut ui_prefs: Option<Option<PathBuf>> = None;
    let mut gallery: Option<String> = None;
    let mut user_data: Option<PathBuf> = None;
    let mut hidden_arg = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" | "-s" => source = Source::Remote(args.next().unwrap_or_else(|| usage())),
            "--demo" => {
                source = Source::Demo(args.next().and_then(|n| n.parse().ok()).unwrap_or(10_000))
            }
            "--prefs" => {
                ui_prefs = Some(Some(PathBuf::from(args.next().unwrap_or_else(|| usage()))))
            }
            "--no-prefs" => ui_prefs = Some(None),
            "--gallery" => gallery = Some(args.next().unwrap_or_else(|| "dark".into())),
            "--user-data" => {
                user_data = Some(PathBuf::from(args.next().unwrap_or_else(|| usage())))
            }
            "--hidden" => hidden_arg = true,
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    let paths = paths::Paths::detect(user_data);
    // the viewer's video player uses the same (bundled) ffmpeg as the server
    pa_ui::video::set_ffmpeg_path(PathBuf::from(server::ffmpeg_path()));
    let script = std::env::var("PA_SCRIPT")
        .ok()
        .filter(|s| !s.trim().is_empty());

    if let Some(theme) = gallery {
        logfile::init(None);
        if let Err(e) = pa_ui::run_gallery(&theme, "en", script) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let embedded = matches!(source, Source::Embedded);
    logfile::init(embedded.then(|| paths.logs.clone()));
    log::info!(
        "start {VERSION} ({}, {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    if embedded {
        let first = instance::acquire(|_msg| {
            if let Some(s) = SHARED.get() {
                s.show_window();
            }
        });
        if matches!(first, instance::Instance::Other) {
            log::info!("already running: asked it to show its window");
            return;
        }
    }

    let prefs = prefs::DesktopPrefs::load(&paths.desktop_json());
    // GPU off (Settings → Desktop): Slint's software renderer, read at start like Electron's switch
    let gpu_active =
        prefs.gpu && std::env::var("SLINT_BACKEND").map_or(true, |b| !b.contains("software"));
    if !prefs.gpu
        && std::env::var_os("SLINT_BACKEND").is_none()
        && cfg!(feature = "software-renderer")
    {
        std::env::set_var("SLINT_BACKEND", "winit-software");
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("pa-worker")
        .enable_all()
        .build()
        .expect("tokio runtime");

    let ui_prefs = ui_prefs.unwrap_or_else(|| Some(paths.ui_prefs()));
    // first start of the Rust app on an Electron profile: the web UI's prefs lived in Chromium's
    // localStorage (electron_prefs.rs); else keep at least the language the shell knew
    if let Some(p) = ui_prefs.as_ref().filter(|p| !p.exists()) {
        let lang = prefs.lang.as_deref().filter(|l| *l == "tr" || *l == "en");
        if let Some(migrated) = electron_prefs::migrate(&paths.user_data, prefs.port) {
            if let Err(e) = migrated.save(p) {
                log::warn!("{}: {e}", p.display());
            }
        } else if let Some(l) = lang {
            let _ = std::fs::create_dir_all(&paths.user_data);
            let _ = std::fs::write(p, format!("{{\"lang\":\"{l}\"}}"));
        }
    }
    let mut shared: Option<Arc<Shared>> = None;
    let backend: Arc<dyn Backend> = match source {
        Source::Remote(url) => match pa_client::Client::new(&url) {
            Ok(c) => HttpBackend::new(c, runtime.handle().clone()),
            Err(e) => fatal(&format!("{url}: {e}")),
        },
        Source::Demo(n) => DemoBackend::new(n),
        Source::Embedded => {
            let server = match runtime.block_on(server::start(&paths, prefs.port, VERSION)) {
                Ok(s) => Arc::new(s),
                Err(e) => fatal(&format!("The gallery server could not start:\n{e}")),
            };
            log::info!("server ready {}", server.url);
            let s = Arc::new(Shared {
                paths: paths.clone(),
                prefs: Mutex::new(prefs.clone()),
                server: Some(server.clone()),
                rt: runtime.handle().clone(),
                version: VERSION.into(),
                gpu_active,
                quitting: AtomicBool::new(false),
                visible: AtomicBool::new(false),
                focused: AtomicBool::new(false),
                window: Mutex::new(None),
                tray_refresh: Mutex::new(None),
                os_scale: Mutex::new(1.0),
                jobs: Mutex::new(Default::default()),
            });
            if prefs.port != Some(server.port) {
                s.prefs.lock().unwrap().port = Some(server.port);
                s.save();
            }
            let _ = SHARED.set(s.clone());
            // in-process server events: taskbar progress, notifications, tray
            let s2 = s.clone();
            std::mem::forget(server.ctx.events.on(move |e| match e {
                pa_core::ServerEvent::Job { job } => s2.on_job(job),
                pa_core::ServerEvent::Devices => s2.refresh_tray(),
                pa_core::ServerEvent::Pair { device } if !s2.focused.load(Ordering::Relaxed) => {
                    let lg = s2.lang();
                    s2.notify(
                        &prefs::l(
                            lg,
                            &format!("📱 {} bağlanmak istiyor", device.name),
                            &format!("📱 {} wants to connect", device.name),
                        ),
                        &prefs::l(
                            lg,
                            "Onaylamak için PeekAround’u aç.",
                            "Open PeekAround to approve.",
                        ),
                        false,
                    );
                }
                _ => {}
            }));
            if prefs.lan.enabled {
                s.start_lan_share();
            }
            let client =
                pa_client::Client::new(&server.url).unwrap_or_else(|e| fatal(&e.to_string()));
            shared = Some(s);
            HttpBackend::new(client, runtime.handle().clone())
        }
    };

    let autoscroll = std::env::var("PA_AUTOSCROLL").ok().map(|v| {
        let mut it = v.split(',');
        AutoScroll {
            px_per_tick: it.next().and_then(|s| s.parse().ok()).unwrap_or(40.0),
            seconds: it.next().and_then(|s| s.parse().ok()).unwrap_or(10.0),
            quit: std::env::var("PA_AUTOSCROLL_QUIT").is_ok_and(|v| v == "1"),
        }
    });
    let opts = UiOptions {
        prefs_path: ui_prefs,
        script,
        autoscroll,
        frame_log: std::env::var("PA_FRAME_LOG").is_ok_and(|v| v == "1"),
    };

    let r = match shared.clone() {
        None => pa_ui::run(backend, runtime.handle().clone(), opts),
        Some(s) => {
            // login launch with "start hidden": tray only
            let hidden = {
                let p = s.prefs.lock().unwrap();
                p.start_hidden && p.start_at_login && hidden_arg
            };
            let host: Rc<dyn pa_ui::host::Host> = Rc::new(DesktopHost { s: s.clone() });
            let s2 = s.clone();
            let integ = pa_ui::Integration {
                host,
                show: !hidden,
                setup: Some(Box::new(move |w: &pa_ui::AppWindow| {
                    setup_window(&s2, w, hidden)
                })),
            };
            pa_ui::run_with(backend, runtime.handle().clone(), opts, integ)
        }
    };

    if let Some(s) = &shared {
        s.quitting.store(true, Ordering::Relaxed);
        s.save();
        if let Some(server) = &s.server {
            let srv = server.clone();
            runtime.block_on(async move {
                let _ = tokio::time::timeout(Duration::from_secs(5), srv.close()).await;
            });
        }
    }
    runtime.shutdown_timeout(Duration::from_millis(500));
    if let Err(e) = r {
        log::error!("ui: {e}");
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    if RESTART.load(Ordering::Relaxed) {
        if let Ok(exe) = std::env::current_exe() {
            let args: Vec<String> = std::env::args()
                .skip(1)
                .filter(|a| a != "--hidden")
                .collect();
            let _ = std::process::Command::new(exe).args(args).spawn();
        }
    }
}

/// window state, zoom, close action, focus tracking, tray, update checks (UI thread, before show)
fn setup_window(s: &Arc<Shared>, w: &pa_ui::AppWindow, hidden: bool) {
    use slint::winit_030::WinitWindowAccessor;
    *s.window.lock().unwrap() = Some(w.as_weak());
    let win = w.window();
    let (bw, bh, _) = s.restored_bounds(&[]);
    win.set_size(slint::LogicalSize::new(bw as f32, bh as f32));

    // close → tray or quit (main.ts 'close' / 'window-all-closed')
    let s2 = s.clone();
    win.on_close_requested(move || {
        s2.save_bounds();
        if !s2.quitting.load(Ordering::Relaxed) && s2.keep_in_tray() {
            let first = {
                let mut p = s2.prefs.lock().unwrap();
                let first = p.tray_hint_shown != Some(true);
                p.tray_hint_shown = Some(true);
                first
            };
            if first {
                s2.save();
                let lg = s2.lang();
                s2.notify(
                    "PeekAround",
                    &prefs::l(
                        lg,
                        "Tepside çalışmaya devam ediyor.",
                        "Still running in the tray.",
                    ),
                    true,
                );
            }
            s2.visible.store(false, Ordering::Relaxed);
            s2.focused.store(false, Ordering::Relaxed);
            s2.refresh_tray();
        } else {
            s2.quitting.store(true, Ordering::Relaxed);
            let _ = slint::quit_event_loop();
        }
        slint::CloseRequestResponse::HideWindow
    });

    // focus + OS scale changes (zoom is applied on top of the OS scale); the window's single winit
    // filter belongs to pa-ui (actions_ui), which forwards events to observers
    let s2 = s.clone();
    pa_ui::observe_window_events(move |ev| match ev {
        slint::winit_030::winit::event::WindowEvent::Focused(f) => {
            s2.focused.store(*f, Ordering::Relaxed);
        }
        slint::winit_030::winit::event::WindowEvent::ScaleFactorChanged {
            scale_factor, ..
        } => {
            *s2.os_scale.lock().unwrap() = *scale_factor as f32;
            let zoom = s2.prefs.lock().unwrap().zoom.unwrap_or(1.0);
            if (zoom - 1.0).abs() > 0.001 {
                let s3 = s2.clone();
                slint::Timer::single_shot(Duration::from_millis(0), move || {
                    desktop::apply_zoom(&s3, zoom)
                });
            }
        }
        _ => {}
    });

    tray::install(s);

    // after the window exists: position (on a connected display), maximized / fullscreen, zoom
    let s2 = s.clone();
    let restore = move || {
        let Some(w) = s2.window.lock().unwrap().clone().and_then(|w| w.upgrade()) else {
            return;
        };
        let win = w.window();
        s2.visible.store(win.is_visible(), Ordering::Relaxed);
        let monitors: Vec<(f64, f64, f64, f64)> = win
            .with_winit_window(|ww| {
                *s2.os_scale.lock().unwrap() = ww.scale_factor() as f32;
                ww.available_monitors()
                    .map(|m| {
                        let sf = m.scale_factor();
                        let p = m.position();
                        let z = m.size();
                        (
                            p.x as f64 / sf,
                            p.y as f64 / sf,
                            z.width as f64 / sf,
                            z.height as f64 / sf,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (_, _, pos) = s2.restored_bounds(&monitors);
        if let Some((x, y)) = pos {
            win.set_position(slint::LogicalPosition::new(x as f32, y as f32));
        }
        let b = s2.prefs.lock().unwrap().bounds.clone().unwrap_or_default();
        if b.maximized == Some(true) {
            win.set_maximized(true);
        }
        if b.fullscreen == Some(true) {
            win.set_fullscreen(true);
        }
        let zoom = s2.prefs.lock().unwrap().zoom.unwrap_or(1.0);
        if (zoom - 1.0).abs() > 0.001 {
            desktop::apply_zoom(&s2, zoom);
        }
        s2.refresh_tray();
    };
    if hidden {
        // the window is created on demand (tray → Show); restore then
        s.refresh_tray();
    }
    slint::Timer::single_shot(Duration::from_millis(50), restore);

    // release checks: 30 s after the start, then every 3 h (each runs at most once a day)
    let s2 = s.clone();
    slint::Timer::single_shot(Duration::from_secs(30), move || s2.maybe_auto_check());
    let s2 = s.clone();
    let t = slint::Timer::default();
    t.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(3 * 3600),
        move || s2.maybe_auto_check(),
    );
    std::mem::forget(t);
}
