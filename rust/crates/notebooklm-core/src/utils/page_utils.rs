//! Page utilities for extracting responses from NotebookLM web UI
//! (Rust rewrite of src/utils/page-utils.ts)
//!
//! ## Phase 4 scope
//! This module provides the constants, data types, and hash helper that can be
//! tested without a browser.  The async functions that actually interact with
//! a `chromiumoxide::Page` are introduced in Phase 5 once the browser crate
//! is available.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ---------------------------------------------------------------------------
// Selectors
// ---------------------------------------------------------------------------

/// CSS selectors to find assistant response elements.
/// Ordered by priority: most specific (NotebookLM-specific) first.
pub const RESPONSE_SELECTORS: &[&str] = &[
    ".to-user-container .message-text-content",
    "[data-message-author='bot']",
    "[data-message-author='assistant']",
    "[data-message-role='assistant']",
    "[data-author='assistant']",
    "[data-renderer*='assistant']",
    "[data-automation-id='response-text']",
    "[data-automation-id='assistant-response']",
    "[data-automation-id='chat-response']",
    "[data-testid*='assistant']",
    "[data-testid*='response']",
    "[aria-live='polite']",
    "[role='listitem'][data-message-author]",
];

/// Primary container selector (most specific for NotebookLM).
pub const PRIMARY_CONTAINER_SELECTOR: &str = ".to-user-container";

/// Inner text-content selector within a primary container.
pub const MESSAGE_TEXT_SELECTOR: &str = ".message-text-content";

/// Spinner / "thinking" indicator selector.
pub const THINKING_SELECTOR: &str = "div.thinking-message";

// ---------------------------------------------------------------------------
// Required stable polls before accepting a response
// ---------------------------------------------------------------------------

/// Number of consecutive identical polls required before treating a response
/// as fully streamed.  **Critical constant** — must match the TypeScript value.
pub const REQUIRED_STABLE_POLLS: usize = 3;

// ---------------------------------------------------------------------------
// Hash helper
// ---------------------------------------------------------------------------

/// Deterministic hash of a string using `DefaultHasher`.
///
/// Used for O(1) comparison of known vs new response texts without storing
/// the full strings.  Within a single process run this is stable; it is NOT
/// guaranteed to be the same across Rust versions or separate runs, which is
/// fine — we only compare hashes within one server invocation.
pub fn hash_string(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// Option types (used by Phase 5 async functions)
// ---------------------------------------------------------------------------

/// Options for `wait_for_latest_answer` (Phase 5).
#[derive(Debug, Clone)]
pub struct WaitOptions {
    /// The question text that was just submitted (used to skip echoes).
    pub question: String,
    /// Maximum wait time in milliseconds (default: 120 000).
    pub timeout_ms: u64,
    /// How often to poll the DOM (default: 1 000).
    pub poll_interval_ms: u64,
    /// Response texts that already existed before the question was asked.
    pub ignore_texts: Vec<String>,
    /// Enable verbose debug logging.
    pub debug: bool,
}

impl Default for WaitOptions {
    fn default() -> Self {
        Self {
            question: String::new(),
            timeout_ms: 120_000,
            poll_interval_ms: 1_000,
            ignore_texts: vec![],
            debug: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Async browser functions (Phase 5)
// ---------------------------------------------------------------------------

use chromiumoxide::Page;

/// Snapshot ALL existing assistant response texts via JS evaluation.
/// Call this BEFORE submitting a new question to capture the baseline.
pub async fn snapshot_all_responses(page: &Page) -> Vec<String> {
    let js = r#"() => {
        const containers = document.querySelectorAll('.to-user-container');
        const texts = [];
        for (const container of containers) {
            const textEl = container.querySelector('.message-text-content');
            if (textEl && textEl.innerText && textEl.innerText.trim()) {
                texts.push(textEl.innerText.trim());
            }
        }
        return texts;
    }"#;

    match page.evaluate_function(js).await {
        Ok(result) => result
            .value()
            .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
            .unwrap_or_default(),
        Err(e) => {
            tracing::warn!("snapshot_all_responses failed: {e}");
            vec![]
        }
    }
}

/// Wait for the latest NEW assistant response using streaming detection.
///
/// Algorithm:
/// 1. Poll the DOM every `opts.poll_interval_ms` ms for a new response
/// 2. A "new" response is one whose hash is not in `opts.ignore_texts`
/// 3. Text must be identical for `REQUIRED_STABLE_POLLS` consecutive polls
/// 4. Returns `None` on timeout
pub async fn wait_for_latest_answer(page: &Page, opts: WaitOptions) -> Option<String> {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(opts.timeout_ms);

    // Seed known-hashes from the pre-ask snapshot
    let mut known_hashes: std::collections::HashSet<u64> = opts
        .ignore_texts
        .iter()
        .filter(|t| !t.trim().is_empty())
        .map(|t| hash_string(t.trim()))
        .collect();

    let sanitized_question = opts.question.trim().to_lowercase();
    let mut last_candidate: Option<String> = None;
    let mut stable_count: usize = 0;
    let mut poll_count: usize = 0;

    while std::time::Instant::now() < deadline {
        poll_count += 1;

        // Skip while NotebookLM is still thinking
        if is_still_thinking(page).await {
            if opts.debug && poll_count % 5 == 0 {
                tracing::debug!("[wait] NotebookLM still thinking...");
            }
            tokio::time::sleep(std::time::Duration::from_millis(opts.poll_interval_ms)).await;
            continue;
        }

        // Fetch all current response texts and pick first NEW one
        if let Some(candidate) = extract_latest_new_text(page, &known_hashes).await {
            let normalized = candidate.trim().to_owned();
            if !normalized.is_empty() {
                // Skip question echo
                if normalized.to_lowercase() == sanitized_question {
                    if opts.debug {
                        tracing::debug!("[wait] skipping question echo");
                    }
                    known_hashes.insert(hash_string(&normalized));
                    tokio::time::sleep(std::time::Duration::from_millis(opts.poll_interval_ms))
                        .await;
                    continue;
                }

                // Streaming detection: count consecutive stable polls
                if last_candidate.as_deref() == Some(normalized.as_str()) {
                    stable_count += 1;
                    if opts.debug && stable_count == REQUIRED_STABLE_POLLS {
                        tracing::debug!(
                            "[wait] stable for {} polls ({} chars)",
                            stable_count,
                            normalized.len()
                        );
                    }
                } else {
                    if opts.debug && last_candidate.is_some() {
                        tracing::debug!(
                            "[wait] text still streaming ({} chars)",
                            normalized.len()
                        );
                    }
                    stable_count = 1;
                    last_candidate = Some(normalized.clone());
                }

                if stable_count >= REQUIRED_STABLE_POLLS {
                    tracing::debug!(
                        "[wait] answer ready ({} chars, {} polls)",
                        normalized.len(),
                        poll_count
                    );
                    return Some(normalized);
                }
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(opts.poll_interval_ms)).await;
    }

    if opts.debug {
        tracing::debug!("[wait] timeout after {} polls", poll_count);
    }
    None
}

// ---------------------------------------------------------------------------
// Private DOM helpers
// ---------------------------------------------------------------------------

async fn is_still_thinking(page: &Page) -> bool {
    let js = r#"() => {
        const el = document.querySelector('div.thinking-message');
        if (!el) return false;
        const style = window.getComputedStyle(el);
        return style.display !== 'none'
            && style.visibility !== 'hidden'
            && el.offsetParent !== null;
    }"#;
    match page.evaluate_function(js).await {
        Ok(result) => result.value().and_then(|v| v.as_bool()).unwrap_or(false),
        Err(_) => false,
    }
}

async fn extract_latest_new_text(
    page: &Page,
    known_hashes: &std::collections::HashSet<u64>,
) -> Option<String> {
    // Fetch all current response texts via JS (one round-trip instead of N)
    let js = r#"() => {
        const containers = document.querySelectorAll('.to-user-container');
        const texts = [];
        for (const container of containers) {
            const textEl = container.querySelector('.message-text-content');
            if (textEl && textEl.innerText && textEl.innerText.trim()) {
                texts.push(textEl.innerText.trim());
            }
        }
        return texts;
    }"#;

    let texts: Vec<String> = match page.evaluate_function(js).await {
        Ok(result) => result
            .value()
            .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
            .unwrap_or_default(),
        Err(_) => return None,
    };

    // Return the first text whose hash is NOT in the known-set
    texts
        .into_iter()
        .find(|t| !known_hashes.contains(&hash_string(t)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Hashing the same string twice must give the same result within one run.
    #[test]
    fn hash_deterministic_within_run() {
        let s = "Hello, NotebookLM!";
        assert_eq!(hash_string(s), hash_string(s));
    }

    /// Two distinct strings must (almost certainly) produce different hashes.
    #[test]
    fn hash_distinct_strings_differ() {
        assert_ne!(hash_string("foo"), hash_string("bar"));
    }

    /// Empty string should hash without panicking.
    #[test]
    fn hash_empty_string() {
        let _ = hash_string("");
    }

    /// Selector list must be non-empty and contain the primary selector.
    #[test]
    fn selectors_non_empty() {
        assert!(!RESPONSE_SELECTORS.is_empty());
        assert!(
            RESPONSE_SELECTORS
                .iter()
                .any(|s| s.contains("to-user-container")),
            "primary selector missing from RESPONSE_SELECTORS"
        );
    }

    /// Stable-poll constant must match the TypeScript value.
    #[test]
    fn required_stable_polls_is_three() {
        assert_eq!(REQUIRED_STABLE_POLLS, 3);
    }

    /// WaitOptions defaults should be sane.
    #[test]
    fn wait_options_defaults() {
        let opts = WaitOptions::default();
        assert_eq!(opts.timeout_ms, 120_000);
        assert_eq!(opts.poll_interval_ms, 1_000);
        assert!(!opts.debug);
    }
}
