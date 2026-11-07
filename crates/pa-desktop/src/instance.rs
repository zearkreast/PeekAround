//! Single instance (Electron `requestSingleInstanceLock` + `second-instance`): the first launch
//! listens on a per-user local socket (Windows named pipe / Linux abstract socket — both vanish
//! with the process, so there is no stale lock); a second launch connects, asks it to show its
//! window and exits.

use std::io::{BufRead, BufReader, Write};

use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};

fn name() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let user: String = user.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("peekaround-desktop-{user}.sock")
}

/// Outcome of [`acquire`].
pub enum Instance {
    /// we are the first one; `on_message(line)` runs on a background thread for each later launch
    First,
    /// another instance runs (it was asked to show its window)
    Other,
}

/// Become the single instance, or hand over to the running one.
pub fn acquire(on_message: impl Fn(String) + Send + 'static) -> Instance {
    let Ok(n) = name().to_ns_name::<GenericNamespaced>() else {
        return Instance::First;
    };
    if let Ok(mut s) = Stream::connect(n.clone()) {
        let _ = s.write_all(b"show\n");
        let _ = s.flush();
        return Instance::Other;
    }
    match ListenerOptions::new().name(n).create_sync() {
        Ok(listener) => {
            std::thread::Builder::new()
                .name("pa-instance".into())
                .spawn(move || {
                    for conn in listener.incoming().flatten() {
                        let mut line = String::new();
                        let _ = BufReader::new(conn).read_line(&mut line);
                        on_message(line.trim().to_string());
                    }
                })
                .ok();
            Instance::First
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Instance::Other,
        Err(e) => {
            log::warn!("single instance lock: {e}");
            Instance::First
        }
    }
}
