//! Stable browser-bridge errors.

use thiserror::Error;

/// Browser bridge result.
pub type Result<T> = std::result::Result<T, BrowserError>;

/// Failures preserved until a product surface maps them.
#[derive(Debug, Error)]
pub enum BrowserError {
    /// Input failed protocol validation.
    #[error("invalid browser request: {0}")]
    InvalidRequest(String),
    /// A requested durable record does not exist.
    #[error("browser bridge record not found")]
    NotFound,
    /// Authentication or signature verification failed.
    #[error("browser authentication failed")]
    AuthenticationFailed,
    /// A browser is not currently connected.
    #[error("the selected browser is not connected")]
    BrowserOffline,
    /// The bounded pending-call capacity has been exhausted.
    #[error("too many browser tool calls are pending")]
    ServerBusy,
    /// A page call exceeded its deadline.
    #[error("the page tool exceeded its time limit")]
    ToolTimeout,
    /// The caller cancelled the page call.
    #[error("the page tool call was cancelled")]
    Cancelled,
    /// The browser document or catalog revision changed.
    #[error("the browser document changed before the tool call completed")]
    StaleDocument,
    /// SQLite persistence failed.
    #[error("browser bridge persistence failed: {0}")]
    Store(#[from] rusqlite::Error),
    /// JSON encoding or decoding failed.
    #[error("browser protocol JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// Internal connection routing ended unexpectedly.
    #[error("browser connection ended unexpectedly")]
    ConnectionClosed,
}

impl BrowserError {
    /// Stable agent-facing error kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::NotFound => "not_found",
            Self::AuthenticationFailed => "auth_failed",
            Self::BrowserOffline => "browser_offline",
            Self::ServerBusy => "server_busy",
            Self::ToolTimeout => "tool_timeout",
            Self::Cancelled => "cancelled",
            Self::StaleDocument => "stale_document",
            Self::Store(_) | Self::Json(_) | Self::ConnectionClosed => "internal_error",
        }
    }
}
