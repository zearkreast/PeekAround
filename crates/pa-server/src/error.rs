//! `HttpError` (mirrors `paths.ts` HttpError + the Fastify error handler): every failure becomes
//! `{ "error": message }` with its status code.

use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// An error with an HTTP status (the TS `HttpError`, or any error with a `statusCode`).
#[derive(Debug, Clone)]
pub struct HttpError {
    pub status: u16,
    /// `error` field of the JSON body
    pub message: String,
    /// extra response headers (e.g. Retry-After)
    pub headers: Vec<(String, String)>,
}

impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            headers: Vec::new(),
        }
    }

    /// add a response header
    pub fn with_header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.to_string(), v.into()));
        self
    }

    /// 500 with the message of an internal error
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::new(500, e.to_string())
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.message)
    }
}

impl std::error::Error for HttpError {}

/// Result of a route handler.
pub type ApiResult<T> = Result<T, HttpError>;

/// `return Err(HttpError::new(status, msg))`
#[macro_export]
macro_rules! bail {
    ($status:expr, $($msg:tt)+) => {
        return Err($crate::error::HttpError::new($status, format!($($msg)+)))
    };
}

impl From<pa_jobs::HttpError> for HttpError {
    fn from(e: pa_jobs::HttpError) -> Self {
        let mut h = HttpError::new(e.status, e.message);
        if let Some(s) = e.retry_after {
            h = h.with_header("Retry-After", s.to_string());
        }
        h
    }
}

impl From<pa_mega::HttpError> for HttpError {
    fn from(e: pa_mega::HttpError) -> Self {
        HttpError::from(
            pa_jobs::HttpError::new(e.status, e.message).with_retry_after(e.retry_after),
        )
    }
}

impl From<HttpError> for pa_jobs::HttpError {
    fn from(e: HttpError) -> Self {
        let ra = e
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
            .and_then(|(_, v)| v.parse().ok());
        pa_jobs::HttpError::new(e.status, e.message).with_retry_after(ra)
    }
}

impl From<rusqlite::Error> for HttpError {
    fn from(e: rusqlite::Error) -> Self {
        HttpError::new(500, e.to_string())
    }
}

/// Node-style message of an fs error (`ENOENT: no such file or directory, stat '<path>'`).
pub fn node_io_message(e: &std::io::Error, syscall: &str, path: &std::path::Path) -> String {
    let (code, text) = match e.kind() {
        std::io::ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        std::io::ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        std::io::ErrorKind::AlreadyExists => ("EEXIST", "file already exists"),
        std::io::ErrorKind::DirectoryNotEmpty => ("ENOTEMPTY", "directory not empty"),
        std::io::ErrorKind::NotADirectory => ("ENOTDIR", "not a directory"),
        std::io::ErrorKind::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        std::io::ErrorKind::CrossesDevices => ("EXDEV", "cross-device link not permitted"),
        _ => ("EIO", "i/o error"),
    };
    format!("{code}: {text}, {syscall} '{}'", path.display())
}

impl From<std::io::Error> for HttpError {
    fn from(e: std::io::Error) -> Self {
        HttpError::new(500, e.to_string())
    }
}

impl From<serde_json::Error> for HttpError {
    fn from(e: serde_json::Error) -> Self {
        HttpError::new(500, e.to_string())
    }
}

impl From<pa_media::MediaError> for HttpError {
    fn from(e: pa_media::MediaError) -> Self {
        HttpError::new(500, e.to_string())
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.message });
        let mut r = crate::http::json_response(&body);
        *r.status_mut() =
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        for (k, v) in self.headers {
            if let (Ok(k), Ok(v)) = (HeaderName::try_from(k), HeaderValue::try_from(v)) {
                r.headers_mut().insert(k, v);
            }
        }
        r
    }
}
