//! Logging utilities (Rust rewrite of src/utils/logger.ts)
//!
//! The TypeScript logger wrote ANSI-coloured lines to stderr via `console.error`.
//! Here we use `tracing` + `tracing-subscriber` with the same stderr target.
//!
//! **Critical**: stdout must remain clean for MCP JSON-RPC.
//! All log output goes to stderr.

use tracing_subscriber::{fmt, EnvFilter};

/// Initialise the global tracing subscriber.
///
/// Call once in `main` (before any `tracing::*!()` calls).
/// Subsequent calls are silently ignored (safe for tests).
pub fn init_tracing() {
    // RUST_LOG env var controls verbosity; default to "info"
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = fmt()
        .with_writer(std::io::stderr) // stdout is reserved for JSON-RPC
        .with_ansi(true)              // ANSI colour codes
        .with_target(false)           // omit module path from log lines
        .with_thread_ids(false)
        .compact()
        .with_env_filter(filter)
        .try_init();                  // silently ignore "already initialised"
}

// ---------------------------------------------------------------------------
// Re-export tracing macros under familiar names (mirrors log.info / log.error)
// ---------------------------------------------------------------------------
//
// Usage in other modules:
//   use crate::utils::logger::{info, warn, error};
//   info!("message {}", value);

pub use tracing::{debug, error, info, warn};
