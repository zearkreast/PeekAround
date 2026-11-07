//! Logging: stderr + `<logs>/main.log`, rotated to `main.old.log` past 1 MB (shell.ts `log`).
//! Level: `PEEK_LOG` / `RUST_LOG` as a single level (error|warn|info|debug|trace); default: info
//! for PeekAround's crates, warn for the rest. The server's `tracing` events arrive through
//! tracing's `log` bridge.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use log::{Level, LevelFilter, Metadata, Record};

const MAX_LOG: u64 = 1024 * 1024;

struct Logger {
    file: Mutex<Option<(PathBuf, std::fs::File, u64)>>,
    ours: LevelFilter,
    others: LevelFilter,
}

fn ours(target: &str) -> bool {
    target.starts_with("pa_") || target.starts_with("peekaround")
}

impl log::Log for Logger {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level()
            <= if ours(m.target()) {
                self.ours
            } else {
                self.others
            }
    }
    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let ts = now_iso();
        let line = format!("{ts} {:<5} {}: {}\n", r.level(), r.target(), r.args());
        let _ = std::io::stderr().write_all(line.as_bytes());
        let mut g = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((path, f, size)) = g.as_mut() {
            if *size > MAX_LOG {
                let old = path.with_file_name("main.old.log");
                let _ = std::fs::rename(&*path, old);
                if let Ok(nf) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&*path)
                {
                    *f = nf;
                    *size = 0;
                }
            }
            if f.write_all(line.as_bytes()).is_ok() {
                *size += line.len() as u64;
            }
        }
    }
    fn flush(&self) {
        if let Some((_, f, _)) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = f.flush();
        }
    }
}

/// UTC ISO-8601 timestamp (`new Date().toISOString()`)
pub fn now_iso() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let ms = d.as_millis() as i64;
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let (y, m, dd) = pa_ui::i18n::civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{dd:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

fn parse(v: &str) -> Option<LevelFilter> {
    Some(match v.trim().to_ascii_lowercase().as_str() {
        "off" => LevelFilter::Off,
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "info" => LevelFilter::Info,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => return None,
    })
}

/// Install the logger (`dir`: None = stderr only).
pub fn init(dir: Option<PathBuf>) {
    let env = std::env::var("PEEK_LOG")
        .ok()
        .or_else(|| std::env::var("RUST_LOG").ok())
        .and_then(|v| parse(&v));
    let file = dir.and_then(|d| {
        std::fs::create_dir_all(&d).ok()?;
        let p = d.join("main.log");
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .ok()?;
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        Some((p, f, size))
    });
    let lg = Logger {
        file: Mutex::new(file),
        ours: env.unwrap_or(LevelFilter::Info),
        others: env
            .map(|e| e.min(LevelFilter::Warn))
            .unwrap_or(LevelFilter::Warn),
    };
    let max = lg.ours.max(lg.others);
    if log::set_boxed_logger(Box::new(lg)).is_ok() {
        log::set_max_level(max);
    }
    let _ = Level::Info;
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso() {
        let s = super::now_iso();
        assert_eq!(s.len(), 24);
        assert!(s.ends_with('Z'));
    }
}
