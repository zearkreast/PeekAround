//! JNI entry points of `app.peekaround.PeekService` (the ":link" process): it starts / stops the
//! phone ↔ PC link and gets its state back (notification text, locks, end of sharing). The OS
//! services of the link (MediaStore thumbnails, video frames, media scanner) are Java methods of
//! the service object.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};

use pa_phone::{LinkClient, LinkStatus, Platform, VideoInfo};

struct Running {
    rt: tokio::runtime::Runtime,
    client: LinkClient,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

struct JavaPlatform {
    vm: JavaVM,
    svc: GlobalRef,
}

impl JavaPlatform {
    fn call<R>(&self, f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
        let mut env = self.vm.attach_current_thread().ok()?;
        let r = env.with_local_frame(16, |env| f(env, self.svc.as_obj()));
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        r.map_err(|e| log::warn!("jni: {e}")).ok()
    }

    fn bytes(&self, name: &str, sig: &str, path: &Path, ints: &[i32]) -> Option<Vec<u8>> {
        self.call(|env, svc| {
            let p = env.new_string(path.to_string_lossy())?;
            let mut args = vec![JValue::Object(&p)];
            args.extend(ints.iter().map(|i| JValue::Int(*i)));
            let o = env.call_method(svc, name, sig, &args)?.l()?;
            if o.is_null() {
                return Ok(None);
            }
            let a = JByteArray::from(o);
            Ok(Some(env.convert_byte_array(&a)?))
        })
        .flatten()
        .filter(|b| !b.is_empty())
    }
}

impl Platform for JavaPlatform {
    fn status(&self, s: &LinkStatus) {
        let json = serde_json::to_string(s).unwrap_or_default();
        self.call(|env, svc| {
            let j = env.new_string(json)?;
            env.call_method(svc, "onLinkState", "(Ljava/lang/String;)V", &[JValue::Object(&j)])
                .map(|_| ())
        });
    }
    fn active(&self, n: usize) {
        self.call(|env, svc| {
            env.call_method(svc, "onLinkActive", "(I)V", &[JValue::Int(n as i32)])
                .map(|_| ())
        });
    }
    fn system_thumb(&self, path: &Path, kind: &str, size: u32) -> Option<Vec<u8>> {
        let video = i32::from(kind == "video");
        self.bytes(
            "systemThumb",
            "(Ljava/lang/String;II)[B",
            path,
            &[video, size as i32],
        )
    }
    fn video_frame(&self, path: &Path, max: u32) -> Option<Vec<u8>> {
        self.bytes("videoFrame", "(Ljava/lang/String;I)[B", path, &[max as i32])
    }
    fn video_strip(&self, path: &Path, n: u32, h: u32) -> Option<Vec<u8>> {
        self.bytes(
            "videoStrip",
            "(Ljava/lang/String;II)[B",
            path,
            &[n as i32, h as i32],
        )
    }
    fn video_info(&self, path: &Path) -> Option<VideoInfo> {
        // "width|height|durationMs|takenMs|location"
        let s = self
            .call(|env, svc| {
                let p = env.new_string(path.to_string_lossy())?;
                let o = env
                    .call_method(
                        svc,
                        "videoInfo",
                        "(Ljava/lang/String;)Ljava/lang/String;",
                        &[JValue::Object(&p)],
                    )?
                    .l()?;
                if o.is_null() {
                    return Ok(String::new());
                }
                let v: String = env.get_string(&JString::from(o))?.into();
                Ok(v)
            })
            .filter(|s| !s.is_empty())?;
        let f: Vec<&str> = s.split('|').collect();
        let num = |i: usize| f.get(i).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        Some(VideoInfo {
            width: num(0).max(0) as u32,
            height: num(1).max(0) as u32,
            duration: num(2) as f64 / 1000.0,
            taken: Some(num(3)).filter(|t| *t > 0),
            location: f.get(4).map(|s| s.to_string()).filter(|s| !s.is_empty()),
        })
    }
    fn scanned(&self, path: &Path) {
        let p = path.to_string_lossy().into_owned();
        self.call(|env, svc| {
            let j = env.new_string(p)?;
            env.call_method(svc, "scanFile", "(Ljava/lang/String;)V", &[JValue::Object(&j)])
                .map(|_| ())
        });
    }
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(Into::into).unwrap_or_default()
}

/// `PeekService.nativeStart(filesDir, service, appVersion, model)`: (re)start the link with
/// the settings in link.json
#[no_mangle]
pub extern "system" fn Java_app_peekaround_PeekService_nativeStart<'l>(
    mut env: JNIEnv<'l>,
    _cls: JClass<'l>,
    files: JString<'l>,
    svc: JObject<'l>,
    app: JString<'l>,
    model: JString<'l>,
) {
    crate::platform::init_logging();
    let files = PathBuf::from(jstr(&mut env, &files));
    let app = jstr(&mut env, &app);
    let model = jstr(&mut env, &model);
    let (Ok(vm), Ok(svc)) = (env.get_java_vm(), env.new_global_ref(svc)) else {
        return;
    };
    let mut running = RUNNING.lock().unwrap();
    if let Some(r) = running.take() {
        r.client.stop();
        r.rt.shutdown_background();
    }
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("pa-link")
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("link runtime: {e}");
            return;
        }
    };
    let platform: Arc<dyn Platform> = Arc::new(JavaPlatform { vm, svc });
    let client = {
        let _g = rt.enter();
        LinkClient::start(&files, platform, &app, &model)
    };
    log::info!("link started ({})", files.display());
    *running = Some(Running { rt, client });
}

/// `PeekService.nativeStop()`
#[no_mangle]
pub extern "system" fn Java_app_peekaround_PeekService_nativeStop<'l>(
    _env: JNIEnv<'l>,
    _cls: JClass<'l>,
) {
    if let Some(r) = RUNNING.lock().unwrap().take() {
        r.client.stop();
        // let the close frame and the "off" state go out
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            r.rt.shutdown_background();
        });
    }
}

/// `PeekService.nativeFoldersChanged()`: link.json's folders were edited
#[no_mangle]
pub extern "system" fn Java_app_peekaround_PeekService_nativeFoldersChanged<'l>(
    _env: JNIEnv<'l>,
    _cls: JClass<'l>,
) {
    if let Some(r) = RUNNING.lock().unwrap().as_ref() {
        r.client.folders_changed();
    }
}

/// `PeekService.nativeReconnect()`: network back / server or name changed
#[no_mangle]
pub extern "system" fn Java_app_peekaround_PeekService_nativeReconnect<'l>(
    _env: JNIEnv<'l>,
    _cls: JClass<'l>,
) {
    if let Some(r) = RUNNING.lock().unwrap().as_ref() {
        r.client.reconnect();
    }
}
