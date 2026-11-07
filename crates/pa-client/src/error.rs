/// Client error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// the server answered with an error status (`{ error }` body message when present)
    #[error("HTTP {status}: {message}")]
    Api {
        /// HTTP status
        status: u16,
        /// message
        message: String,
    },
    /// connection failed / aborted
    #[error("network error: {0}")]
    Network(String),
    /// unexpected response body
    #[error("bad response: {0}")]
    Decode(String),
    /// invalid server URL
    #[error("invalid url: {0}")]
    Url(String),
}

impl Error {
    /// HTTP status of an API error (0 for network errors, like the web UI's `ApiError`)
    pub fn status(&self) -> u16 {
        match self {
            Error::Api { status, .. } => *status,
            _ => 0,
        }
    }
}

/// Client result.
pub type Result<T> = std::result::Result<T, Error>;
