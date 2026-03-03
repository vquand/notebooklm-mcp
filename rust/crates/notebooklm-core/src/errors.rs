//! Custom error types (Rust rewrite of src/errors.ts)

use thiserror::Error;

/// Raised when NotebookLM rate limit is exceeded.
///
/// Free accounts: ~50 queries/day.
/// Handling: re_auth (switch Google account) or wait until tomorrow.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct RateLimitError {
    pub message: String,
}

impl RateLimitError {
    pub fn new() -> Self {
        Self {
            message: "NotebookLM rate limit reached (50 queries/day for free accounts)".into(),
        }
    }

    pub fn with_message(msg: impl Into<String>) -> Self {
        Self { message: msg.into() }
    }
}

impl Default for RateLimitError {
    fn default() -> Self {
        Self::new()
    }
}

/// Raised when authentication fails.
///
/// `suggest_cleanup` is set when a full data cleanup might fix the issue
/// (e.g., after upgrading from an old installation).
#[derive(Debug, Error)]
#[error("{message}")]
pub struct AuthenticationError {
    pub message: String,
    pub suggest_cleanup: bool,
}

impl AuthenticationError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { message: msg.into(), suggest_cleanup: false }
    }

    pub fn with_cleanup_hint(msg: impl Into<String>) -> Self {
        Self { message: msg.into(), suggest_cleanup: true }
    }
}

/// Browser / CDP session was closed unexpectedly and needs re-initialisation.
#[derive(Debug, Error)]
#[error("Browser session closed: {message}")]
pub struct SessionClosedError {
    pub message: String,
}

impl SessionClosedError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { message: msg.into() }
    }
}

/// Check whether an anyhow error originated from a closed browser/page.
pub fn is_closed_error(e: &anyhow::Error) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("has been closed")
        || msg.contains("target closed")
        || msg.contains("browser has been closed")
        || msg.contains("connection refused")
        || msg.contains("context destroyed")
}
