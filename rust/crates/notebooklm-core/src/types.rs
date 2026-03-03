//! Global type definitions (Rust rewrite of src/types.ts)

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Session information returned by the API (mirrors SessionInfo in types.ts)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    /// Unix timestamp (ms) when session was created
    pub created_at: i64,
    /// Unix timestamp (ms) of last activity
    pub last_activity: i64,
    pub age_seconds: u64,
    pub inactive_seconds: u64,
    pub message_count: usize,
    pub notebook_url: String,
}

// ---------------------------------------------------------------------------
// Ask question result
// ---------------------------------------------------------------------------

/// Inline session snapshot embedded in `AskQuestionResult`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoSnapshot {
    pub age_seconds: u64,
    pub message_count: usize,
    pub last_activity: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryStatus {
    Success,
    Error,
}

/// Result from asking a question (mirrors AskQuestionResult in types.ts)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskQuestionResult {
    pub status: QueryStatus,
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub notebook_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_info: Option<SessionInfoSnapshot>,
}

// ---------------------------------------------------------------------------
// Generic tool result wrapper
// ---------------------------------------------------------------------------

/// MCP tool call response wrapper (mirrors ToolResult<T> in types.ts)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolResult {
    pub fn ok(data: serde_json::Value) -> Self {
        Self { success: true, data: Some(data), error: None }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self { success: false, data: None, error: Some(msg.into()) }
    }
}

// ---------------------------------------------------------------------------
// Typing / wait options
// ---------------------------------------------------------------------------

/// Options for human-like typing (mirrors TypingOptions in types.ts)
#[derive(Debug, Clone, Default)]
pub struct TypingOptions {
    /// Words per minute (overrides config if set)
    pub wpm: Option<u32>,
    pub with_typos: bool,
}

/// Options for waiting for a NotebookLM answer (mirrors WaitForAnswerOptions)
#[derive(Debug, Clone)]
pub struct WaitForAnswerOptions {
    pub question: Option<String>,
    pub timeout_ms: u64,
    pub poll_interval_ms: u64,
    pub ignore_texts: Vec<String>,
    pub debug: bool,
}

impl Default for WaitForAnswerOptions {
    fn default() -> Self {
        Self {
            question: None,
            timeout_ms: 120_000,
            poll_interval_ms: 1_000,
            ignore_texts: vec![],
            debug: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Progress callback
// ---------------------------------------------------------------------------

/// Async progress notification callback (mirrors ProgressCallback in types.ts)
///
/// `(message, progress, total) -> Future<Output = ()>`
pub type ProgressFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
pub type ProgressCallback =
    Arc<dyn Fn(String, Option<f64>, Option<f64>) -> ProgressFuture + Send + Sync>;

/// Convenience: build a no-op progress callback
pub fn noop_progress() -> ProgressCallback {
    Arc::new(|_, _, _| Box::pin(async {}))
}
