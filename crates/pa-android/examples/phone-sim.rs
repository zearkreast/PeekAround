//! The Android app's UI process on the desktop (winit instead of android-activity, JNI calls
//! stubbed): `cargo run -p pa-android --example phone-sim -- <files dir>`.
//! env: PA_SETUP_SCRIPT / PA_SCRIPT (automation), PA_PHONE_STORAGE (stand-in for /storage/emulated/0)

fn main() {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: phone-sim <files dir>");
        std::process::exit(2);
    };
    peekaround_android::run(std::path::Path::new(&dir));
}
