//! "Is there a PeekAround server at this address?" (1.x `features/android/probe.ts`) and the
//! LAN discovery of the first-run screen (mDNS `_peekaround._tcp`, announced by pa-server).

use std::future::Future;
use std::time::Duration;

/// the desktop app's default LAN share port (Settings → Network)
pub const LAN_PORT: u16 = 1313;
/// the Docker / server default (and the desktop default before 1.3.1)
pub const SERVER_PORT: u16 = 8080;

/// why a probe failed — the message says what to check
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// not an http(s) address
    Invalid,
    /// nothing answered (address, same Wi-Fi, LAN share on, PC firewall)
    Unreachable,
    /// it answered with an error status
    Refused(u16),
    /// some other web server
    NotPeekAround,
}

/// a found server
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// base URL that answered
    pub url: String,
    /// its host name
    pub hostname: String,
}

/// what one `GET /api/info` gave
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// network error / timeout
    Down,
    /// non-2xx
    Status(u16),
    /// 2xx: `name` / `hostname` of the JSON body, None when it was not ServerInfo JSON
    Info(Option<(String, String)>),
}

fn normalize_base(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw.trim()).ok()?;
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some()
    {
        return None;
    }
    u.host_str().filter(|h| !h.is_empty())?;
    let path = u.path().trim_end_matches('/');
    Some(format!("{}{}", u.origin().ascii_serialization(), path))
}

/// Addresses to try for what was typed: without a scheme http:// is added; without a port the
/// address as typed, the desktop app's LAN port and the Docker port. Empty when not http(s).
pub fn candidate_urls(input: &str) -> Vec<String> {
    let mut raw = input.trim().to_string();
    let lower = raw.to_ascii_lowercase();
    if raw.is_empty() || lower == "http://" || lower == "https://" || lower == "http:/" {
        return vec![];
    }
    let has_scheme = raw
        .find("://")
        .map(|i| {
            let s = &raw[..i];
            !s.is_empty()
                && s.chars().next().unwrap().is_ascii_alphabetic()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        })
        .unwrap_or(false);
    if !has_scheme {
        raw = format!("http://{raw}");
    }
    let Some(base) = normalize_base(&raw) else {
        return vec![];
    };
    let Ok(u) = url::Url::parse(&base) else {
        return vec![];
    };
    if u.port().is_some() || (u.path() != "/" && !u.path().is_empty()) {
        return vec![base];
    }
    let mut out = vec![base];
    for p in [LAN_PORT, SERVER_PORT] {
        let mut v = u.clone();
        if v.set_port(Some(p)).is_ok() {
            if let Some(b) = normalize_base(v.as_str()) {
                out.push(b);
            }
        }
    }
    out
}

/// [`probe`] with the request injected (tests)
pub async fn probe_with<F, Fut>(input: &str, get_info: F) -> Result<Found, ProbeError>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Answer>,
{
    let urls = candidate_urls(input);
    if urls.is_empty() {
        return Err(ProbeError::Invalid);
    }
    let mut result = ProbeError::Unreachable;
    for url in urls {
        match get_info(url.clone()).await {
            Answer::Info(Some((name, hostname))) if name == "PeekAround" => {
                return Ok(Found { url, hostname })
            }
            // an answer beats "nothing there" when telling the user what went wrong
            Answer::Info(_) => result = ProbeError::NotPeekAround,
            Answer::Status(s) => result = ProbeError::Refused(s),
            Answer::Down => {}
        }
    }
    Err(result)
}

/// GET /api/info on every candidate address (6 s each)
pub async fn probe(input: &str) -> Result<Found, ProbeError> {
    probe_with(input, |url| async move {
        let Ok(c) = pa_client::Client::new(&url) else {
            return Answer::Down;
        };
        match tokio::time::timeout(Duration::from_secs(6), c.info()).await {
            Err(_) => Answer::Down,
            Ok(Ok(i)) => Answer::Info(Some((i.name, i.hostname))),
            Ok(Err(pa_client::Error::Api { status, .. })) => Answer::Status(status),
            Ok(Err(pa_client::Error::Decode(_))) => Answer::Info(None),
            Ok(Err(_)) => Answer::Down,
        }
    })
    .await
}

/// PeekAround servers announced on the LAN within `timeout` (name, base URL)
pub fn discover(timeout: Duration) -> Vec<(String, String)> {
    let Ok(d) = mdns_sd::ServiceDaemon::new() else {
        return vec![];
    };
    let Ok(rx) = d.browse("_peekaround._tcp.local.") else {
        let _ = d.shutdown();
        return vec![];
    };
    let end = std::time::Instant::now() + timeout;
    let mut out: Vec<(String, String)> = vec![];
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        let Ok(ev) = rx.recv_timeout(left) else { break };
        if let mdns_sd::ServiceEvent::ServiceResolved(info) = ev {
            let Some(ip) = info
                .get_addresses()
                .iter()
                .find(|a| a.is_ipv4())
                .copied()
            else {
                continue;
            };
            let url = format!("http://{}:{}", ip, info.get_port());
            let name = info
                .get_fullname()
                .split("._peekaround")
                .next()
                .unwrap_or("")
                .to_string();
            if !out.iter().any(|(_, u)| *u == url) {
                out.push((if name.is_empty() { url.clone() } else { name }, url));
            }
        }
    }
    let _ = d.stop_browse("_peekaround._tcp.local.");
    let _ = d.shutdown();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[test]
    fn candidates_from_what_was_typed() {
        assert_eq!(
            candidate_urls("http://192.168.1.20:8080/"),
            ["http://192.168.1.20:8080"]
        );
        assert_eq!(
            candidate_urls(" 192.168.1.20:8080 "),
            ["http://192.168.1.20:8080"]
        );
        assert_eq!(
            candidate_urls("192.168.1.20"),
            [
                "http://192.168.1.20",
                "http://192.168.1.20:1313",
                "http://192.168.1.20:8080"
            ]
        );
        assert_eq!(
            candidate_urls("https://photos.example.com"),
            [
                "https://photos.example.com",
                "https://photos.example.com:1313",
                "https://photos.example.com:8080"
            ]
        );
        assert_eq!(
            candidate_urls("http://nas.local/peek"),
            ["http://nas.local/peek"]
        );
        for bad in ["", "http://", "ftp://x", "http://user:pw@x", "javascript:alert(1)"] {
            assert!(candidate_urls(bad).is_empty(), "{bad}");
        }
    }

    fn run<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn fake(routes: &[(&str, Answer)]) -> impl Fn(String) -> std::future::Ready<Answer> {
        let m: HashMap<String, Answer> = routes
            .iter()
            .map(|(k, v)| (format!("{k}/api/info"), v.clone()))
            .collect();
        move |url| std::future::ready(m.get(&format!("{url}/api/info")).cloned().unwrap_or(Answer::Down))
    }

    fn info() -> Answer {
        Answer::Info(Some(("PeekAround".into(), "DESKTOP-AB12".into())))
    }

    #[test]
    fn probe_accepts_peekaround_and_tries_the_lan_port() {
        let calls = RefCell::new(vec![]);
        let f = fake(&[("http://192.168.1.20:8080", info())]);
        let r = run(probe_with("http://192.168.1.20:8080", |u| {
            calls.borrow_mut().push(u.clone());
            f(u)
        }));
        assert_eq!(
            r,
            Ok(Found {
                url: "http://192.168.1.20:8080".into(),
                hostname: "DESKTOP-AB12".into()
            })
        );
        assert_eq!(calls.into_inner(), ["http://192.168.1.20:8080"]);
        let r = run(probe_with(
            "192.168.1.20",
            fake(&[("http://192.168.1.20:1313", info())]),
        ));
        assert_eq!(r.unwrap().url, "http://192.168.1.20:1313");
    }

    #[test]
    fn probe_errors_say_what_to_check() {
        assert_eq!(
            run(probe_with("http://192.168.1.20:8080", fake(&[]))),
            Err(ProbeError::Unreachable)
        );
        assert_eq!(
            run(probe_with(
                "http://192.168.1.1",
                fake(&[("http://192.168.1.1", Answer::Info(None))])
            )),
            Err(ProbeError::NotPeekAround)
        );
        assert_eq!(
            run(probe_with(
                "http://pc:8080",
                fake(&[("http://pc:8080", Answer::Status(421))])
            )),
            Err(ProbeError::Refused(421))
        );
        assert_eq!(
            run(probe_with("nonsense://", fake(&[]))),
            Err(ProbeError::Invalid)
        );
    }
}
