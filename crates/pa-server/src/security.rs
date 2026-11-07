//! Browser-facing protection for an unauthenticated LAN gallery (mirrors `security.ts`):
//! Host allow-list (DNS rebinding → 421), Origin allow-list (CORS + CSRF → 403), Sec-Fetch-Site,
//! and the @fastify/rate-limit compatible limiter (global 6000/min, FS 240/min, HEAVY 60/min,
//! tiles 20000/min, per route and client IP).

use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// the Android app's WebView origin (capacitor androidScheme http)
const APP_ORIGINS: [&str; 3] = [
    "http://localhost",
    "https://localhost",
    "capacitor://localhost",
];
/// local-only TLDs / names that a remote attacker cannot own
const LOCAL_SUFFIXES: [&str; 6] = [
    ".local",
    ".lan",
    ".home",
    ".internal",
    ".home.arpa",
    ".localhost",
];

/// "Host" header → bare lower-case host name (no port, no IPv6 brackets)
pub fn host_name(host: Option<&str>) -> String {
    let Some(host) = host else {
        return String::new();
    };
    let h = pa_core::js::trim(host).to_lowercase();
    if let Some(rest) = h.strip_prefix('[') {
        return match rest.find(']') {
            Some(end) if end > 0 => rest[..end].to_string(),
            _ => h,
        };
    }
    match h.find(':') {
        Some(c) if !h[c + 1..].contains(':') => h[..c].to_string(),
        _ => h,
    }
}

/// `net.isIP`
fn is_ip(s: &str) -> bool {
    s.parse::<IpAddr>().is_ok()
}

/// Host allow-list check.
pub fn is_allowed_host(host: Option<&str>, extra: &[String]) -> bool {
    if extra.iter().any(|e| e == "*") {
        return true;
    }
    let name = host_name(host);
    if name.is_empty() || is_ip(&name) || name == "localhost" || !name.contains('.') {
        return true;
    }
    if LOCAL_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return true;
    }
    let hn = crate::config::hostname().to_lowercase();
    if name == hn || name.starts_with(&format!("{hn}.")) {
        return true;
    }
    extra
        .iter()
        .any(|e| name == *e || (e.starts_with("*.") && name.ends_with(&e[1..])))
}

/// 127.0.0.1 / ::1 / ::ffff:127.0.0.1
pub fn is_loopback_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.octets() == [127, 0, 0, 1],
        IpAddr::V6(v) => {
            v.is_loopback()
                || v.to_ipv4_mapped()
                    .is_some_and(|m| m.octets() == [127, 0, 0, 1])
        }
    }
}

/// Origin allow-list check (`loopback_client`: the request comes from this machine).
pub fn is_allowed_origin(
    origin: Option<&str>,
    host: Option<&str>,
    extra: &[String],
    loopback_client: bool,
) -> bool {
    let Some(origin) = origin else { return true };
    if origin.is_empty() {
        return true;
    }
    if origin == "null" {
        return false;
    }
    let o = origin.to_lowercase();
    if APP_ORIGINS.contains(&o.as_str()) {
        return true;
    }
    if extra.iter().any(|e| *e == o || e == "*") {
        return true;
    }
    let Ok(u) = url::Url::parse(&o) else {
        return false;
    };
    if u.scheme() != "http" && u.scheme() != "https" {
        return false;
    }
    let hostname = match u.host() {
        Some(url::Host::Ipv6(a)) => format!("[{a}]"),
        Some(h) => h.to_string(),
        None => String::new(),
    };
    if loopback_client && ["localhost", "127.0.0.1", "[::1]", "::1"].contains(&hostname.as_str()) {
        return true;
    }
    let uhost = match u.port() {
        Some(p) => format!("{hostname}:{p}"),
        None => hostname,
    };
    host.is_some_and(|h| !h.is_empty() && uhost == h.to_lowercase())
}

/// GET / HEAD / OPTIONS
pub fn is_safe_method(m: &str) -> bool {
    matches!(m, "GET" | "HEAD" | "OPTIONS")
}

/// Rate limit class of a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Limit {
    /// the generous global limit (6000 / min, one store for every such route)
    Global,
    /// file-system routes (240 / min, per route)
    Fs,
    /// expensive one-shot work (60 / min, per route)
    Heavy,
    /// map tiles (20000 / min, per route)
    Tile,
}

impl Limit {
    /// max per minute
    pub fn max(self) -> u64 {
        match self {
            Limit::Global => 6000,
            Limit::Fs => 240,
            Limit::Heavy => 60,
            Limit::Tile => 20_000,
        }
    }
}

struct Entry {
    current: u64,
    start: Instant,
}

/// Outcome of one counted request.
#[derive(Debug, Clone, Copy)]
pub struct RateState {
    pub max: u64,
    pub remaining: u64,
    pub reset_s: u64,
    pub exceeded: bool,
}

/// LocalStore of @fastify/rate-limit: fixed one-minute windows per key.
#[derive(Default)]
pub struct RateLimiter {
    stores: Mutex<HashMap<String, Entry>>,
}

const WINDOW: Duration = Duration::from_secs(60);

impl RateLimiter {
    /// count a request of `ip` against `store` (route id for per-route limits, "" for the global one)
    pub fn hit(&self, store: &str, ip: &IpAddr, max: u64) -> RateState {
        let key = format!("{store}\u{0}{ip}");
        let now = Instant::now();
        let mut m = self.stores.lock();
        if m.len() > 20_000 {
            m.retain(|_, e| now.duration_since(e.start) < WINDOW);
        }
        let e = m.entry(key).or_insert(Entry {
            current: 0,
            start: now,
        });
        if e.current == 0 || now.duration_since(e.start) >= WINDOW {
            e.current = 1;
            e.start = now;
        } else {
            e.current += 1;
        }
        let ttl = WINDOW.saturating_sub(now.duration_since(e.start));
        let reset_s = ttl.as_millis().div_ceil(1000) as u64;
        RateState {
            max,
            remaining: max.saturating_sub(e.current),
            reset_s,
            exceeded: e.current > max,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert_eq!(host_name(Some("Example.com:8080")), "example.com");
        assert_eq!(host_name(Some("[::1]:8080")), "::1");
        assert_eq!(host_name(Some("192.168.1.5")), "192.168.1.5");
        for h in [
            Some("localhost:47213"),
            Some("127.0.0.1:8080"),
            Some("192.168.1.20:8080"),
            Some("[::1]:1"),
            Some("nas:8080"),
            Some("nas.local"),
            Some("pc.lan"),
            Some("box.home.arpa"),
            None,
        ] {
            assert!(is_allowed_host(h, &[]), "{h:?}");
        }
        for h in ["evil.example.com", "attacker.net:8080", "127.0.0.1.nip.io"] {
            assert!(!is_allowed_host(Some(h), &[]), "{h}");
        }
        assert!(is_allowed_host(
            Some("photos.example.com"),
            &["photos.example.com".into()]
        ));
        assert!(is_allowed_host(
            Some("a.example.com"),
            &["*.example.com".into()]
        ));
        assert!(is_allowed_host(Some("anything.com"), &["*".into()]));
    }

    #[test]
    fn origins() {
        assert!(is_allowed_origin(None, Some("127.0.0.1:8080"), &[], false));
        assert!(is_allowed_origin(
            Some("http://127.0.0.1:8080"),
            Some("127.0.0.1:8080"),
            &[],
            false
        ));
        assert!(is_allowed_origin(
            Some("http://192.168.1.5:8080"),
            Some("192.168.1.5:8080"),
            &[],
            false
        ));
        assert!(is_allowed_origin(
            Some("http://localhost"),
            Some("192.168.1.5:8080"),
            &[],
            false
        ));
        assert!(!is_allowed_origin(
            Some("https://evil.example"),
            Some("127.0.0.1:8080"),
            &[],
            false
        ));
        assert!(!is_allowed_origin(
            Some("null"),
            Some("127.0.0.1:8080"),
            &[],
            false
        ));
        assert!(!is_allowed_origin(
            Some("http://localhost:5173"),
            Some("127.0.0.1:8787"),
            &[],
            false
        ));
        assert!(is_allowed_origin(
            Some("http://localhost:5173"),
            Some("127.0.0.1:8787"),
            &[],
            true
        ));
        assert!(is_allowed_origin(
            Some("https://my.app"),
            Some("127.0.0.1:8787"),
            &["https://my.app".into()],
            false
        ));
    }
}
