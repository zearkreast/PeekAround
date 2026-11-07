//! The desktop shell state shared by the UI thread, the tray and background tasks, and the
//! [`pa_ui::host::Host`] implementation (Electron main.ts IPC handlers).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pa_server::Server;
use pa_ui::host::{FileFilter, Host, HostFut, LanState, ShellPatch, ShellState, UpdateInfo};
use slint::ComponentHandle;

use crate::paths::Paths;
use crate::prefs::{l, Bounds, DesktopPrefs};

/// Electron ZOOMS
pub const ZOOMS: [f64; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// "open at login" is offered on Windows and Linux (and macOS)
pub const LOGIN_SUPPORTED: bool = cfg!(any(windows, target_os = "linux", target_os = "macos"));

/// State shared across threads.
pub struct Shared {
    pub paths: Paths,
    /// desktop.json
    pub prefs: Mutex<DesktopPrefs>,
    /// the embedded server (None: `--server URL` mode)
    pub server: Option<Arc<Server>>,
    pub rt: tokio::runtime::Handle,
    pub version: String,
    /// renderer this process started with
    pub gpu_active: bool,
    pub quitting: AtomicBool,
    pub visible: AtomicBool,
    pub focused: AtomicBool,
    /// the window (upgrade on the UI thread only)
    pub window: Mutex<Option<slint::Weak<pa_ui::AppWindow>>>,
    /// redraws the tray (set by tray.rs)
    pub tray_refresh: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// the OS scale factor of the window (zoom multiplies it)
    pub os_scale: Mutex<f32>,
    /// active jobs → taskbar progress
    pub jobs: Mutex<std::collections::HashMap<String, pa_core::Job>>,
}

impl Shared {
    /// save desktop.json
    pub fn save(&self) {
        let p = self.prefs.lock().unwrap().clone();
        p.save(&self.paths.desktop_json());
    }

    /// tray / notification language
    pub fn lang(&self) -> &'static str {
        self.prefs.lock().unwrap().lang()
    }

    /// LAN sharing runs
    pub fn lan_on(&self) -> bool {
        self.server.as_ref().is_some_and(|s| s.ctx.lan.lock().0)
    }

    /// LAN addresses (while sharing)
    pub fn lan_urls(&self) -> Vec<String> {
        match &self.server {
            Some(s) => {
                let (on, port) = *s.ctx.lan.lock();
                if on {
                    pa_server::config::lan_urls(port)
                } else {
                    vec![]
                }
            }
            None => vec![],
        }
    }

    /// a paired phone is connected
    pub fn phone_active(&self) -> bool {
        self.server.as_ref().is_some_and(|s| {
            s.ctx
                .devices
                .list(&s.ctx, None)
                .map(|l| l.iter().any(|d| d.paired && d.online))
                .unwrap_or(false)
        })
    }

    /// closing the window keeps the app running in the tray
    pub fn keep_in_tray(&self) -> bool {
        let ca = self.prefs.lock().unwrap().close_action.clone();
        ca == "tray" || (ca == "auto" && (self.lan_on() || self.phone_active()))
    }

    /// the tray icon is needed
    pub fn tray_needed(&self) -> bool {
        if self.server.is_none() || self.quitting.load(Ordering::Relaxed) {
            return false;
        }
        self.lan_on()
            || self.phone_active()
            || !self.visible.load(Ordering::Relaxed)
            || self.prefs.lock().unwrap().close_action == "tray"
    }

    pub fn refresh_tray(&self) {
        if let Some(f) = &*self.tray_refresh.lock().unwrap() {
            f();
        }
    }

    /// show (create) the window — from any thread
    pub fn show_window(self: &Arc<Self>) {
        let me = self.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let w = me.window.lock().unwrap().clone();
            if let Some(w) = w.and_then(|w| w.upgrade()) {
                let win = w.window();
                if win.is_minimized() {
                    win.set_minimized(false);
                }
                let _ = w.show();
                me.visible.store(true, Ordering::Relaxed);
                use slint::winit_030::WinitWindowAccessor;
                win.with_winit_window(|ww| ww.focus_window());
                me.refresh_tray();
            }
        });
    }

    /// quit the app (tray "Quit", close with closeAction quit) — from any thread
    pub fn quit(self: &Arc<Self>) {
        let me = self.clone();
        let _ = slint::invoke_from_event_loop(move || {
            me.save_bounds();
            me.quitting.store(true, Ordering::Relaxed);
            let _ = slint::quit_event_loop();
        });
    }

    /// remember the window size / position (UI thread)
    pub fn save_bounds(&self) {
        let w = self.window.lock().unwrap().clone();
        let Some(w) = w.and_then(|w| w.upgrade()) else {
            return;
        };
        let win = w.window();
        let sf = win.scale_factor();
        let maximized = win.is_maximized();
        let fullscreen = win.is_fullscreen();
        let mut p = self.prefs.lock().unwrap();
        let mut b = p.bounds.clone().unwrap_or_default();
        // keep the normal bounds while maximized / fullscreen
        if !maximized && !fullscreen && !win.is_minimized() {
            let size = win.size().to_logical(sf);
            let pos = win.position().to_logical(sf);
            b.width = size.width as f64;
            b.height = size.height as f64;
            b.x = Some(pos.x as f64);
            b.y = Some(pos.y as f64);
        }
        b.maximized = Some(maximized);
        b.fullscreen = Some(fullscreen);
        if b.width > 0.0 {
            p.bounds = Some(b);
        }
        drop(p);
        self.save();
    }

    /// announce a newer release once (a toast through the UI)
    pub fn pending_update(&self) -> Option<UpdateInfo> {
        let p = self.prefs.lock().unwrap();
        let u = p.last_update.clone()?;
        if !u.newer
            || u.latest.is_none()
            || p.notified_version.as_deref() == u.latest.as_deref()
            || u.current != self.version
        {
            return None;
        }
        Some(u)
    }

    /// a system notification (Electron `notify`): only when enabled (or forced)
    pub fn notify(&self, title: &str, body: &str, force: bool) {
        if !force && !self.prefs.lock().unwrap().notify {
            return;
        }
        crate::os::notify(title, body);
    }

    /// job events → taskbar progress + "job finished" notifications for long jobs in the background
    pub fn on_job(&self, j: &pa_core::Job) {
        use pa_core::JobStatus::*;
        let done = matches!(j.status, Done | Error | Canceled);
        let mut jobs = self.jobs.lock().unwrap();
        let prev = jobs.contains_key(&j.id);
        if done {
            jobs.remove(&j.id);
            let age = crate::now_ms() - j.created_at;
            if prev && j.status != Canceled && age > 8000 && !self.focused.load(Ordering::Relaxed) {
                let lg = self.lang();
                if j.status == Done {
                    self.notify(&l(lg, "İş tamamlandı", "Job finished"), &j.title, false);
                } else {
                    let body = match &j.error {
                        Some(e) => format!("{} — {e}", j.title),
                        None => j.title.clone(),
                    };
                    self.notify(&l(lg, "İş başarısız", "Job failed"), &body, false);
                }
            }
        } else {
            jobs.insert(j.id.clone(), j.clone());
        }
        let active: Vec<&pa_core::Job> = jobs
            .values()
            .filter(|j| matches!(j.status, Running | Queued))
            .collect();
        let v = if active.is_empty() {
            None
        } else {
            let sum: f64 = active
                .iter()
                .map(|j| if j.status == Running { j.progress } else { 0.0 })
                .sum();
            Some((sum / active.len() as f64).clamp(0.02, 1.0) as f32)
        };
        drop(jobs);
        self.set_progress(v);
    }

    /// taskbar progress (UI thread hop)
    pub fn set_progress(&self, v: Option<f32>) {
        let w = self.window.lock().unwrap().clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w.and_then(|w| w.upgrade()) {
                crate::os::taskbar_progress(w.window(), v);
            }
        });
    }

    /// apply the start-at-login setting
    pub fn apply_login_item(&self) {
        if !LOGIN_SUPPORTED {
            return;
        }
        let on = self.prefs.lock().unwrap().start_at_login;
        if let Err(e) = crate::os::set_autostart(on) {
            log::warn!("login item: {e}");
        }
    }

    /// LAN sharing that was on at the last run; retried with backoff (main.ts `startLanShare`)
    pub fn start_lan_share(self: &Arc<Self>) {
        let Some(server) = self.server.clone() else {
            return;
        };
        let me = self.clone();
        self.rt.spawn(async move {
            for attempt in 0..7u32 {
                let (enabled, port) = {
                    let p = me.prefs.lock().unwrap();
                    (p.lan.enabled, p.lan.port)
                };
                if !enabled || me.quitting.load(Ordering::Relaxed) || me.lan_on() {
                    return;
                }
                match server.set_lan(true, Some(port)).await {
                    Ok(_) => {
                        if attempt > 0 {
                            log::info!("LAN share started (attempt {})", attempt + 1);
                        }
                        me.refresh_tray();
                        return;
                    }
                    Err(e) => {
                        log::warn!(
                            "LAN share could not start on port {port} (attempt {}): {e}",
                            attempt + 1
                        );
                        let wait = (2000u64 << attempt).min(60_000);
                        tokio::time::sleep(Duration::from_millis(wait)).await;
                    }
                }
            }
        });
    }

    /// switch LAN sharing (settings, tray)
    pub async fn set_lan(self: &Arc<Self>, enabled: bool, port: u16) -> LanState {
        let Some(server) = self.server.clone() else {
            return LanState {
                error: Some("not supported".into()),
                ..Default::default()
            };
        };
        match server.set_lan(enabled, Some(port)).await {
            Ok(info) => {
                {
                    let mut p = self.prefs.lock().unwrap();
                    p.lan.enabled = info.enabled;
                    p.lan.port = port;
                }
                self.save();
                self.refresh_tray();
                LanState {
                    enabled: info.enabled,
                    port,
                    urls: info.urls,
                    error: None,
                }
            }
            Err(e) => {
                self.refresh_tray();
                LanState {
                    enabled: false,
                    port,
                    urls: vec![],
                    error: Some(e.to_string()),
                }
            }
        }
    }

    /// release check; `manual`: from Settings (no toast offer, marks the version seen)
    pub async fn run_update_check(self: &Arc<Self>, manual: bool) -> UpdateInfo {
        let u = crate::update::check(&self.version).await;
        {
            let mut p = self.prefs.lock().unwrap();
            if let Some(e) = &u.error {
                log::warn!("update check failed: {e}");
            } else {
                p.last_update_check = Some(u.checked_at);
            }
            p.last_update = Some(u.clone());
            if manual && u.newer {
                p.notified_version = u.latest.clone();
            }
        }
        self.save();
        if !manual {
            self.offer_update();
        }
        u
    }

    /// the UI shows a toast with the release link (once per version)
    pub fn offer_update(&self) {
        let Some(u) = self.pending_update() else {
            return;
        };
        let latest = u.latest.clone();
        let w = self.window.lock().unwrap().clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w.and_then(|w| w.upgrade()) {
                pa_ui::announce_update(&w, &u);
            }
        });
        self.prefs.lock().unwrap().notified_version = latest;
        self.save();
    }

    /// daily check (main.ts `maybeAutoCheck`)
    pub fn maybe_auto_check(self: &Arc<Self>) {
        let (auto, last) = {
            let p = self.prefs.lock().unwrap();
            (p.auto_update, p.last_update_check.unwrap_or(0))
        };
        if auto && !self.quitting.load(Ordering::Relaxed) && crate::now_ms() - last > 24 * 3_600_000
        {
            let me = self.clone();
            self.rt.spawn(async move {
                me.run_update_check(false).await;
            });
        }
    }

    fn shell_state(&self) -> ShellState {
        let p = self.prefs.lock().unwrap();
        ShellState {
            start_at_login: LOGIN_SUPPORTED && p.start_at_login,
            start_hidden: p.start_hidden,
            close_action: p.close_action.clone(),
            gpu: p.gpu,
            notify: p.notify,
            auto_update: p.auto_update,
            login_supported: LOGIN_SUPPORTED,
            gpu_active: self.gpu_active,
            version: self.version.clone(),
            last_update: p.last_update.clone(),
        }
    }

    /// window bounds to restore (Electron `restoredBounds`): ≥ 880×560, the position only while
    /// it is still on a connected display
    pub fn restored_bounds(
        &self,
        monitors: &[(f64, f64, f64, f64)],
    ) -> (f64, f64, Option<(f64, f64)>) {
        let b = self.prefs.lock().unwrap().bounds.clone().unwrap_or(Bounds {
            width: 1400.0,
            height: 900.0,
            ..Default::default()
        });
        let (w, h) = (
            if b.width > 0.0 { b.width } else { 1400.0 }.max(880.0),
            if b.height > 0.0 { b.height } else { 900.0 }.max(560.0),
        );
        let pos = match (b.x, b.y) {
            (Some(x), Some(y)) => monitors
                .iter()
                .any(|&(mx, my, mw, mh)| {
                    x < mx + mw - 80.0 && x + w > mx + 80.0 && y >= my - 40.0 && y < my + mh - 80.0
                })
                .then_some((x, y)),
            _ => None,
        };
        (w, h, pos)
    }
}

/// [`Host`] for the UI (main.ts IPC handlers).
pub struct DesktopHost {
    pub s: Arc<Shared>,
}

fn rfd_filters(mut d: rfd::AsyncFileDialog, filters: &[FileFilter]) -> rfd::AsyncFileDialog {
    for f in filters {
        d = d.add_filter(&f.name, &f.extensions);
    }
    d
}

impl Host for DesktopHost {
    fn platform(&self) -> &'static str {
        if cfg!(windows) {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        }
    }
    fn is_desktop(&self) -> bool {
        self.s.server.is_some()
    }
    fn pick_folders(&self, title: &str) -> HostFut<Vec<PathBuf>> {
        let d = rfd::AsyncFileDialog::new().set_title(title);
        Box::pin(async move {
            d.pick_folders()
                .await
                .map(|v| v.into_iter().map(|h| h.path().to_path_buf()).collect())
                .unwrap_or_default()
        })
    }
    fn pick_files(&self, title: &str, filters: Vec<FileFilter>) -> HostFut<Vec<PathBuf>> {
        let d = rfd_filters(rfd::AsyncFileDialog::new().set_title(title), &filters);
        Box::pin(async move {
            d.pick_files()
                .await
                .map(|v| v.into_iter().map(|h| h.path().to_path_buf()).collect())
                .unwrap_or_default()
        })
    }
    fn save_file(&self, title: &str, name: &str) -> HostFut<Option<PathBuf>> {
        let mut d = rfd::AsyncFileDialog::new()
            .set_title(title)
            .set_file_name(name);
        if let Some(dl) = dirs::download_dir() {
            d = d.set_directory(dl);
        }
        Box::pin(async move { d.save_file().await.map(|h| h.path().to_path_buf()) })
    }
    fn open_url(&self, url: &str) {
        // only plain web links leave the app
        if url.starts_with("https://") || url.starts_with("http://") {
            if let Err(e) = open::that_detached(url) {
                log::warn!("open {url}: {e}");
            }
        }
    }
    fn show_in_folder(&self, path: &str) {
        crate::os::show_in_folder(std::path::Path::new(path));
    }
    fn open_path(&self, which: &str) -> Result<(), String> {
        let dir = match which {
            "data" => self.s.paths.user_data.clone(),
            "logs" => self.s.paths.logs.clone(),
            _ => return Err("Unknown folder".into()),
        };
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        open::that_detached(&dir).map_err(|e| e.to_string())
    }
    fn lan(&self) -> Option<LanState> {
        self.s.server.as_ref()?;
        let port = self.s.prefs.lock().unwrap().lan.port;
        Some(LanState {
            enabled: self.s.lan_on(),
            port,
            urls: self.s.lan_urls(),
            error: None,
        })
    }
    fn set_lan(&self, enabled: bool, port: u16) -> HostFut<LanState> {
        let s = self.s.clone();
        Box::pin(async move {
            if port == 0 {
                return LanState {
                    enabled: s.lan_on(),
                    port,
                    error: Some("Invalid port".into()),
                    ..Default::default()
                };
            }
            s.set_lan(enabled, port).await
        })
    }
    fn shell(&self) -> Option<ShellState> {
        self.s.server.as_ref()?;
        Some(self.s.shell_state())
    }
    fn set_shell(&self, patch: ShellPatch) -> Option<ShellState> {
        let login = patch.start_at_login.is_some() || patch.start_hidden.is_some();
        let close = patch.close_action.is_some();
        let auto = patch.auto_update == Some(true);
        {
            let mut p = self.s.prefs.lock().unwrap();
            if let Some(v) = patch.start_at_login {
                p.start_at_login = v;
            }
            if let Some(v) = patch.start_hidden {
                p.start_hidden = v;
            }
            if let Some(v) = patch
                .close_action
                .filter(|v| matches!(v.as_str(), "auto" | "tray" | "quit"))
            {
                p.close_action = v;
            }
            if let Some(v) = patch.gpu {
                p.gpu = v;
            }
            if let Some(v) = patch.notify {
                p.notify = v;
            }
            if let Some(v) = patch.auto_update {
                p.auto_update = v;
            }
        }
        self.s.save();
        if login {
            self.s.apply_login_item();
        }
        if close {
            self.s.refresh_tray();
        }
        if auto {
            self.s.maybe_auto_check();
        }
        self.shell()
    }
    fn restart(&self) {
        crate::RESTART.store(true, Ordering::Relaxed);
        self.s.quit();
    }
    fn check_update(&self) -> HostFut<UpdateInfo> {
        let s = self.s.clone();
        Box::pin(async move { s.run_update_check(true).await })
    }
    fn take_pending_update(&self) -> Option<UpdateInfo> {
        let u = self.s.pending_update()?;
        self.s.prefs.lock().unwrap().notified_version = u.latest.clone();
        self.s.save();
        Some(u)
    }
    fn set_lang(&self, lang: &str) {
        if lang != "tr" && lang != "en" {
            return;
        }
        let changed = {
            let mut p = self.s.prefs.lock().unwrap();
            let c = p.lang.as_deref() != Some(lang);
            p.lang = Some(lang.into());
            c
        };
        if changed {
            self.s.save();
            self.s.refresh_tray();
        }
    }
    fn zoom(&self, dir: i32) -> f32 {
        let cur = self.s.prefs.lock().unwrap().zoom.unwrap_or(1.0);
        let next = match dir.signum() {
            1 => ZOOMS
                .iter()
                .copied()
                .find(|z| *z > cur + 0.01)
                .unwrap_or(2.0),
            -1 => ZOOMS
                .iter()
                .rev()
                .copied()
                .find(|z| *z < cur - 0.01)
                .unwrap_or(0.5),
            _ => 1.0,
        };
        self.s.prefs.lock().unwrap().zoom = Some(next);
        self.s.save();
        apply_zoom(&self.s, next);
        next as f32
    }
    fn toggle_fullscreen(&self) {
        let w = self.s.window.lock().unwrap().clone();
        if let Some(w) = w.and_then(|w| w.upgrade()) {
            let win = w.window();
            win.set_fullscreen(!win.is_fullscreen());
        }
    }
    fn notify(&self, title: &str, body: &str) {
        if !self.s.focused.load(Ordering::Relaxed) {
            self.s.notify(title, body, false);
        }
    }
    fn progress(&self, value: Option<f32>) {
        self.s.set_progress(value);
    }
}

/// zoom = the OS scale factor × the factor (UI thread)
pub fn apply_zoom(s: &Shared, zoom: f64) {
    let w = s.window.lock().unwrap().clone();
    if let Some(w) = w.and_then(|w| w.upgrade()) {
        let os = *s.os_scale.lock().unwrap();
        w.window()
            .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: os * zoom as f32,
            });
    }
}
