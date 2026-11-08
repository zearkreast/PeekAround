//! Android APIs used by the UI process: Java methods of `app.peekaround.MainActivity`
//! (crates/pa-android/android/app/src/main/java/app/peekaround/MainActivity.java) called through
//! JNI on the activity object android-activity hands to `ndk-context`. Off Android: stubs.

use std::path::PathBuf;

/// shared storage root ("/storage/emulated/0")
pub fn storage_root() -> PathBuf {
    #[cfg(target_os = "android")]
    if let Some(s) = jni_impl::call_string("storageRoot") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    #[cfg(target_os = "android")]
    return PathBuf::from("/storage/emulated/0");
    #[cfg(not(target_os = "android"))]
    dirs::home_dir().unwrap_or_default()
}

/// "tr" | "en" from the system language
pub fn system_lang() -> &'static str {
    #[cfg(target_os = "android")]
    let l = jni_impl::call_string("systemLanguage").unwrap_or_default();
    #[cfg(not(target_os = "android"))]
    let l = std::env::var("LANG").unwrap_or_default();
    if l.to_ascii_lowercase().starts_with("tr") {
        "tr"
    } else {
        "en"
    }
}

/// "All files access" (MANAGE_EXTERNAL_STORAGE, Android 11+; READ/WRITE_EXTERNAL_STORAGE before)
pub fn has_all_files() -> bool {
    #[cfg(target_os = "android")]
    return jni_impl::call_bool("hasAllFilesAccess").unwrap_or(false);
    #[cfg(not(target_os = "android"))]
    true
}

/// open the system page / dialog that grants [`has_all_files`]
pub fn request_all_files() {
    #[cfg(target_os = "android")]
    jni_impl::call_void("requestAllFilesAccess");
}

/// POST_NOTIFICATIONS (Android 13+) — the sharing notification needs it
pub fn request_notifications() {
    #[cfg(target_os = "android")]
    jni_impl::call_void("requestNotifications");
}

/// a short system toast
pub fn toast(text: &str) {
    #[cfg(target_os = "android")]
    jni_impl::call_str_void("toast", text);
    #[cfg(not(target_os = "android"))]
    log::info!("toast: {text}");
}

/// open a link in the browser
pub fn open_url(url: &str) {
    #[cfg(target_os = "android")]
    jni_impl::call_str_void("openUrl", url);
    #[cfg(not(target_os = "android"))]
    let _ = url;
}

/// clipboard
pub fn copy_text(text: &str) {
    #[cfg(target_os = "android")]
    jni_impl::call_str_void("copyText", text);
    #[cfg(not(target_os = "android"))]
    let _ = text;
}

/// let the gallery apps see a file the app wrote (MediaScannerConnection)
pub fn scan_file(path: &str) {
    #[cfg(target_os = "android")]
    jni_impl::call_str_void("scanFile", path);
    #[cfg(not(target_os = "android"))]
    let _ = path;
}

/// start PeekService (phone folder sharing; it reads link.json)
pub fn start_link_service() -> bool {
    #[cfg(target_os = "android")]
    return jni_impl::call_bool("startLinkService").unwrap_or(false);
    #[cfg(not(target_os = "android"))]
    false
}

/// stop PeekService
pub fn stop_link_service() {
    #[cfg(target_os = "android")]
    jni_impl::call_void("stopLinkService");
}

/// relaunch the app (after "Change connection")
pub fn restart() {
    #[cfg(target_os = "android")]
    jni_impl::call_void("restartApp");
    #[cfg(not(target_os = "android"))]
    let _ = slint::quit_event_loop();
}

/// the UI loop ended: close the activity (and with it the UI process)
pub fn finish() {
    #[cfg(target_os = "android")]
    jni_impl::call_void("finishApp");
}

/// device name for the phone link (Settings.Global.DEVICE_NAME, else maker + model)
pub fn device_name() -> String {
    #[cfg(target_os = "android")]
    return jni_impl::call_string("deviceName").unwrap_or_else(|| "Android".into());
    #[cfg(not(target_os = "android"))]
    "Android".into()
}

/// maker + model
pub fn device_model() -> String {
    #[cfg(target_os = "android")]
    return jni_impl::call_string("deviceModel").unwrap_or_default();
    #[cfg(not(target_os = "android"))]
    String::new()
}

/// logcat for the `log` crate (tag "PeekAround"); panics go there too
#[cfg(target_os = "android")]
pub fn init_logging() {
    jni_impl::init_logging();
}

#[cfg(target_os = "android")]
pub(crate) mod jni_impl {
    use std::ffi::CString;
    use std::sync::Once;

    use jni::objects::{JObject, JString, JValue};
    use jni::JavaVM;

    #[link(name = "log")]
    extern "C" {
        fn __android_log_write(prio: i32, tag: *const std::ffi::c_char, text: *const std::ffi::c_char)
            -> i32;
    }

    struct Logcat;

    impl log::Log for Logcat {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= log::Level::Info || m.target().starts_with("pa_")
        }
        fn log(&self, r: &log::Record) {
            if !self.enabled(r.metadata()) {
                return;
            }
            write(r.level(), &format!("{}: {}", r.target(), r.args()));
        }
        fn flush(&self) {}
    }

    pub(crate) fn write(level: log::Level, msg: &str) {
        let prio = match level {
            log::Level::Error => 6,
            log::Level::Warn => 5,
            log::Level::Info => 4,
            log::Level::Debug => 3,
            log::Level::Trace => 2,
        };
        let tag = c"PeekAround";
        let text = CString::new(msg.replace('\0', " ")).unwrap_or_default();
        unsafe {
            __android_log_write(prio, tag.as_ptr(), text.as_ptr());
        }
    }

    pub(crate) fn init_logging() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let _ = log::set_logger(&Logcat);
            log::set_max_level(log::LevelFilter::Debug);
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                write(log::Level::Error, &format!("panic: {info}"));
                prev(info);
            }));
        });
    }

    fn with_activity<R>(f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
        let ctx = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
        // the android_main thread stays attached: free the local refs of every call
        let r = env.with_local_frame(16, |env| f(env, &activity));
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        // the activity reference belongs to android-activity
        std::mem::forget(activity);
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!("jni: {e}");
                None
            }
        }
    }

    pub(crate) fn call_void(name: &str) {
        with_activity(|env, a| env.call_method(a, name, "()V", &[]).map(|_| ()));
    }

    pub(crate) fn call_bool(name: &str) -> Option<bool> {
        with_activity(|env, a| env.call_method(a, name, "()Z", &[])?.z())
    }

    pub(crate) fn call_str_void(name: &str, arg: &str) {
        with_activity(|env, a| {
            let s = env.new_string(arg)?;
            env.call_method(a, name, "(Ljava/lang/String;)V", &[JValue::Object(&s)])
                .map(|_| ())
        });
    }

    pub(crate) fn call_string(name: &str) -> Option<String> {
        with_activity(|env, a| {
            let o = env
                .call_method(a, name, "()Ljava/lang/String;", &[])?
                .l()?;
            if o.is_null() {
                return Ok(String::new());
            }
            let s = JString::from(o);
            let v: String = env.get_string(&s)?.into();
            Ok(v)
        })
    }
}
