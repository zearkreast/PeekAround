//! The HTTP-facing error of every PeekAround service crate (pa-jobs, pa-link, pa-remote):
//! `HttpError(status, message)` of the TypeScript server, plus MEGA's `Retry-After`.

use std::fmt;

/// An error that maps 1:1 onto an HTTP response `{ "error": message }` with `status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
    /// seconds for a `Retry-After` header (MEGA quota / rate limits)
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
    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(400, message)
    }
    pub fn with_retry_after(mut self, s: Option<u64>) -> Self {
        self.retry_after = s;
        self
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HttpError {}

/// Why a job runner stopped. The message becomes `Job.error` (cut to 500 UTF-16 units); a runner
/// whose cancel token fired always ends as `canceled`, whatever it returns.
#[derive(Debug, Clone, PartialEq)]
pub struct JobFailure {
    pub message: String,
    /// the HTTP status when the failure came from an [`HttpError`] (503 device offline, 429 quota…)
    pub status: Option<u16>,
}

impl JobFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
        }
    }
}

impl fmt::Display for JobFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for JobFailure {}

impl From<HttpError> for JobFailure {
    fn from(e: HttpError) -> Self {
        Self {
            message: e.message,
            status: Some(e.status),
        }
    }
}

impl From<std::io::Error> for JobFailure {
    fn from(e: std::io::Error) -> Self {
        Self::new(e.to_string())
    }
}

impl From<pa_media::jobs::JobError> for JobFailure {
    fn from(e: pa_media::jobs::JobError) -> Self {
        Self::new(e.to_string())
    }
}

impl From<String> for JobFailure {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for JobFailure {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}
