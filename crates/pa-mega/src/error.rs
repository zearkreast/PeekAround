//! MEGA errors and their mapping to the server's HTTP errors (mirrors `megaError` in
//! `remote/mega.ts`; messages keep megajs' wording so the same classification rules apply).

use std::fmt;

/// An error the server answers with (status + English message, optional `Retry-After` seconds).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct HttpError {
    pub status: u16,
    pub message: String,
    pub retry_after: Option<u64>,
}

impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            retry_after: None,
        }
    }
}

/// megajs' API error texts (`ERRORS`), keyed by the positive code
pub fn api_error_text(code: i32) -> String {
    let t = match -code {
        1 => "EINTERNAL (-1): An internal error has occurred. Please submit a bug report, detailing the exact circumstances in which this error occurred.",
        2 => "EARGS (-2): You have passed invalid arguments to this command.",
        3 => "EAGAIN (-3): A temporary congestion or server malfunction prevented your request from being processed. No data was altered. Retried 4 times.",
        4 => "ERATELIMIT (-4): You have exceeded your command weight per time quota. Please wait a few seconds, then try again (this should never happen in sane real-life applications).",
        5 => "EFAILED (-5): The upload failed. Please restart it from scratch.",
        6 => "ETOOMANY (-6): Too many concurrent IP addresses are accessing this upload target URL.",
        7 => "ERANGE (-7): The upload file packet is out of range or not starting and ending on a chunk boundary.",
        8 => "EEXPIRED (-8): The upload target URL you are trying to access has expired. Please request a fresh one.",
        9 => "ENOENT (-9): Object (typically, node or user) not found. Wrong password?",
        10 => "ECIRCULAR (-10): Circular linkage attempted",
        11 => "EACCESS (-11): Access violation (e.g., trying to write to a read-only share)",
        12 => "EEXIST (-12): Trying to create an object that already exists",
        13 => "EINCOMPLETE (-13): Trying to access an incomplete resource",
        14 => "EKEY (-14): A decryption operation failed (never returned by the API)",
        15 => "ESID (-15): Invalid or expired user session, please relogin",
        16 => "EBLOCKED (-16): User blocked",
        17 => "EOVERQUOTA (-17): Request over quota",
        18 => "ETEMPUNAVAIL (-18): Resource temporarily not available, please try again later",
        19 => "ETOOMANYCONNECTIONS (-19)",
        24 => "EGOINGOVERQUOTA (-24)",
        25 => "EROLLEDBACK (-25)",
        26 => "EMFAREQUIRED (-26): Multi-Factor Authentication Required",
        27 => "EMASTERONLY (-27)",
        28 => "EBUSINESSPASTDUE (-28)",
        29 => "EPAYWALL (-29): ODQ paywall state",
        400 => "ETOOERR (-400)",
        401 => "ESHAREROVERQUOTA (-401)",
        _ => return format!("MEGA error {code}"),
    };
    t.to_string()
}

pub const EAGAIN: i32 = -3;
pub const ERATELIMIT: i32 = -4;
pub const EFAILED: i32 = -5;
pub const ENOENT: i32 = -9;
pub const EACCESS: i32 = -11;
pub const EKEY: i32 = -14;
pub const ESID: i32 = -15;
pub const EBLOCKED: i32 = -16;
pub const EOVERQUOTA: i32 = -17;
pub const EMFAREQUIRED: i32 = -26;

#[derive(Debug, Clone, PartialEq)]
pub enum MegaError {
    /// a negative API answer
    Api(i32),
    /// transfer quota (509, EOVERQUOTA with `tl`); seconds until it resets when MEGA says
    Quota { seconds: Option<u64> },
    /// a non-2xx HTTP status of a transfer / API host
    Http(u16),
    /// connection / DNS / TLS / timeout problems
    Network(String),
    /// the link key cannot decrypt the node
    Key(String),
    /// a node inside a folder link does not exist
    NodeNotFound,
    /// MEGA answered something unexpected
    Invalid(String),
    /// downloaded data does not match the node's MAC
    Mac,
    /// the caller canceled
    Aborted,
    /// local errors (I/O, …)
    Other(String),
    /// an error that already is an HTTP answer (bad link, bad range)
    Http4xx(HttpError),
}

impl MegaError {
    pub fn code(&self) -> Option<i32> {
        match self {
            MegaError::Api(c) => Some(*c),
            _ => None,
        }
    }
    pub fn is_session_error(&self) -> bool {
        self.code() == Some(ESID)
    }
    pub fn is_aborted(&self) -> bool {
        matches!(self, MegaError::Aborted)
    }
    /// seconds until the transfer quota resets, when known
    pub fn time_limit(&self) -> Option<u64> {
        match self {
            MegaError::Quota { seconds } => *seconds,
            _ => None,
        }
    }
    /// congestion / rate limit: back off for a moment (EAGAIN / ERATELIMIT)
    pub fn is_congestion(&self) -> bool {
        matches!(self.code(), Some(EAGAIN) | Some(ERATELIMIT))
    }
}

impl fmt::Display for MegaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MegaError::Api(c) => f.write_str(&api_error_text(*c)),
            MegaError::Quota { seconds } => write!(
                f,
                "EOVERQUOTA (-17): Bandwidth limit reached: {} seconds until it resets",
                seconds.unwrap_or(0)
            ),
            MegaError::Http(s) => write!(f, "MEGA returned a {s} status code"),
            MegaError::Network(m) => write!(f, "fetch failed: {m}"),
            MegaError::Key(m) => f.write_str(m),
            MegaError::NodeNotFound => f.write_str("Node (file or folder) not found in folder"),
            MegaError::Invalid(m) => f.write_str(m),
            MegaError::Mac => f.write_str("MAC verification failed"),
            MegaError::Aborted => f.write_str("aborted"),
            MegaError::Other(m) => f.write_str(m),
            MegaError::Http4xx(e) => f.write_str(&e.message),
        }
    }
}

impl std::error::Error for MegaError {}

impl From<reqwest::Error> for MegaError {
    fn from(e: reqwest::Error) -> Self {
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg.push_str(": ");
            msg.push_str(&s.to_string());
            src = s.source();
        }
        MegaError::Network(msg)
    }
}

impl From<std::io::Error> for MegaError {
    fn from(e: std::io::Error) -> Self {
        MegaError::Other(e.to_string())
    }
}

impl From<HttpError> for MegaError {
    fn from(e: HttpError) -> Self {
        MegaError::Http4xx(e)
    }
}

fn contains_any(s: &str, words: &[&str]) -> bool {
    words.iter().any(|w| s.contains(w))
}

/// Friendly HTTP errors for MEGA failures (`megaError`).
pub fn http_error(e: &MegaError) -> HttpError {
    if let MegaError::Http4xx(h) = e {
        return h.clone();
    }
    let msg = e.to_string();
    let seconds = e.time_limit().or_else(|| {
        static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"(\d+) seconds until it resets").unwrap()
        });
        RE.captures(&msg).and_then(|c| c[1].parse().ok())
    });
    let quota = matches!(e, MegaError::Quota { .. } | MegaError::Http(509))
        || contains_any(
            &msg,
            &[
                "EOVERQUOTA",
                "EGOINGOVERQUOTA",
                "ESHAREROVERQUOTA",
                "Bandwidth limit",
                "ETOOMANYCONNECTIONS",
                "ERATELIMIT",
            ],
        )
        || regex_word_509(&msg);
    if quota {
        let s = seconds.filter(|s| *s > 0);
        let wait = s
            .map(|s| format!(" (retry in about {} min)", s.div_ceil(60).max(1)))
            .unwrap_or_default();
        return HttpError {
            status: 429,
            message: format!("MEGA bandwidth quota exceeded, try again later{wait}"),
            retry_after: s,
        };
    }
    if msg.contains("EBLOCKED") {
        return HttpError::new(404, "This MEGA link has been taken down");
    }
    if matches!(e, MegaError::NodeNotFound)
        || contains_any(&msg, &["ENOENT", "not found in folder", "EACCESS"])
    {
        return HttpError::new(404, "MEGA link not found (removed, or the link is wrong)");
    }
    if matches!(e, MegaError::Key(_))
        || contains_any(
            &msg,
            &[
                "EKEY",
                "could not be decrypted",
                "key isn't defined",
                "wrong key length",
            ],
        )
    {
        return HttpError::new(400, "Invalid MEGA link key");
    }
    if contains_any(&msg, &["ETEMPUNAVAIL", "EAGAIN"]) {
        return HttpError::new(503, "MEGA is temporarily unavailable, try again later");
    }
    let lower = msg.to_lowercase();
    if matches!(e, MegaError::Network(_))
        || contains_any(
            &lower,
            &[
                "fetch failed",
                "enotfound",
                "econnreset",
                "etimedout",
                "eai_again",
                "network",
            ],
        )
    {
        return HttpError::new(502, "Cannot reach MEGA");
    }
    let short: String = msg.chars().take(200).collect();
    HttpError::new(502, format!("MEGA error: {short}"))
}

fn regex_word_509(s: &str) -> bool {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\b509\b").unwrap());
    RE.is_match(s)
}

/// The remote-folder status code of an HTTP error (`errorCode` in folders.ts).
pub fn remote_error_code(err: &HttpError) -> &'static str {
    match err.status {
        429 => "quota",
        _ if err.message.contains("taken down") => "blocked",
        404 => "not-found",
        400 => "key",
        502 if err.message.to_lowercase().contains("reach") => "network",
        503 => "unavailable",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_errors_like_the_ts_server() {
        let q = http_error(&MegaError::Quota {
            seconds: Some(1800),
        });
        assert_eq!(q.status, 429);
        assert_eq!(
            q.message,
            "MEGA bandwidth quota exceeded, try again later (retry in about 30 min)"
        );
        assert_eq!(q.retry_after, Some(1800));
        assert!(http_error(&MegaError::Api(EOVERQUOTA))
            .message
            .starts_with("MEGA bandwidth quota exceeded"));
        assert_eq!(http_error(&MegaError::Http(509)).status, 429);
        assert_eq!(
            http_error(&MegaError::Api(ENOENT)).message,
            "MEGA link not found (removed, or the link is wrong)"
        );
        assert_eq!(
            http_error(&MegaError::Key(
                "Attributes could not be decrypted with provided key.".into()
            ))
            .message,
            "Invalid MEGA link key"
        );
        assert_eq!(http_error(&MegaError::Api(EKEY)).status, 400);
        assert_eq!(
            http_error(&MegaError::Api(EBLOCKED)).message,
            "This MEGA link has been taken down"
        );
        assert_eq!(http_error(&MegaError::Api(EAGAIN)).status, 503);
        assert_eq!(http_error(&MegaError::Api(ERATELIMIT)).status, 429);
        assert_eq!(
            http_error(&MegaError::Network("connection refused".into())).message,
            "Cannot reach MEGA"
        );
        assert_eq!(http_error(&MegaError::NodeNotFound).status, 404);
        assert_eq!(
            http_error(&MegaError::Http(500)).message,
            "MEGA error: MEGA returned a 500 status code"
        );
        assert_eq!(
            remote_error_code(&http_error(&MegaError::Api(EBLOCKED))),
            "blocked"
        );
        assert_eq!(
            remote_error_code(&http_error(&MegaError::Network("x".into()))),
            "network"
        );
    }
}
