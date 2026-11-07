//! Small OS integrations: notifications, taskbar progress (Windows), autostart, "show in folder".

use std::path::Path;

pub fn notify(title: &str, body: &str) {
    let mut n = notify_rust::Notification::new();
    n.summary(title).body(body).appname("PeekAround");
    #[cfg(all(unix, not(target_os = "macos")))]
    n.icon("peekaround");
    if let Err(e) = n.show() {
        log::warn!("notification: {e}");
    }
}

/// "open at login" with `--hidden` (main.ts `applyLoginItem`): Windows HKCU Run key, Linux
/// ~/.config/autostart/PeekAround.desktop, macOS launch agent
pub fn set_autostart(on: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let al = auto_launch::AutoLaunchBuilder::new()
        .set_app_name("PeekAround")
        .set_app_path(&exe.to_string_lossy())
        .set_args(&["--hidden"])
        .build()
        .map_err(|e| e.to_string())?;
    let r = if on {
        al.enable()
    } else if al.is_enabled().unwrap_or(false) {
        al.disable()
    } else {
        Ok(())
    };
    r.map_err(|e| e.to_string())
}

/// reveal a file in the file manager
pub fn show_in_folder(p: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe")
            .arg(format!("/select,{}", p.display()))
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let dir = if p.is_dir() {
            p
        } else {
            p.parent().unwrap_or(p)
        };
        if let Err(e) = open::that_detached(dir) {
            log::warn!("show in folder: {e}");
        }
    }
}

/// taskbar progress (Windows ITaskbarList3; elsewhere a no-op)
pub fn taskbar_progress(window: &slint::Window, value: Option<f32>) {
    #[cfg(windows)]
    win::progress(window, value);
    #[cfg(not(windows))]
    let _ = (window, value);
}

#[cfg(windows)]
mod win {
    use std::cell::Cell;
    use std::ffi::c_void;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };

    const CLSID_TASKBAR_LIST: GUID = GUID::from_u128(0x56FDF344_FD6D_11d0_958A_006097C9A090);
    const IID_ITASKBAR_LIST3: GUID = GUID::from_u128(0xea1afb91_9e28_4b86_90e9_9e9f8a5eefaf);

    type Hwnd = *mut c_void;
    #[repr(C)]
    struct Vtbl {
        _qi: usize,
        _add_ref: usize,
        _release: usize,
        hr_init: unsafe extern "system" fn(*mut c_void) -> i32,
        _add_tab: usize,
        _delete_tab: usize,
        _activate_tab: usize,
        _set_active_alt: usize,
        _mark_fullscreen: usize,
        set_progress_value: unsafe extern "system" fn(*mut c_void, Hwnd, u64, u64) -> i32,
        set_progress_state: unsafe extern "system" fn(*mut c_void, Hwnd, i32) -> i32,
    }

    thread_local! {
        static TB: Cell<*mut c_void> = const { Cell::new(std::ptr::null_mut()) };
    }

    fn taskbar() -> Option<*mut c_void> {
        TB.with(|tb| {
            if tb.get().is_null() {
                unsafe {
                    CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
                    let mut p: *mut c_void = std::ptr::null_mut();
                    let hr = CoCreateInstance(
                        &CLSID_TASKBAR_LIST,
                        std::ptr::null_mut(),
                        CLSCTX_INPROC_SERVER,
                        &IID_ITASKBAR_LIST3,
                        &mut p,
                    );
                    if hr < 0 || p.is_null() {
                        return None;
                    }
                    let vt = *(p as *const *const Vtbl);
                    ((*vt).hr_init)(p);
                    tb.set(p);
                }
            }
            Some(tb.get())
        })
    }

    pub fn progress(window: &slint::Window, value: Option<f32>) {
        let Some(hwnd) = window
            .with_winit_window(|w| match w.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get() as Hwnd),
                _ => None,
            })
            .flatten()
        else {
            return;
        };
        let Some(tb) = taskbar() else { return };
        unsafe {
            let vt = *(tb as *const *const Vtbl);
            match value {
                None => {
                    ((*vt).set_progress_state)(tb, hwnd, 0);
                }
                Some(v) => {
                    ((*vt).set_progress_state)(tb, hwnd, 2);
                    ((*vt).set_progress_value)(tb, hwnd, (v.clamp(0.0, 1.0) * 1000.0) as u64, 1000);
                }
            }
        }
    }
}
