//! Browser Session
//! (Rust rewrite of src/session/browser-session.ts)
//!
//! Represents a single tab (`Page`) in the shared persistent Chrome instance.
//! Handles navigation, auth-checking, cookie injection, human-like typing
//! and streaming response detection.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use chromiumoxide::Page;
use tokio::sync::Mutex;

use crate::auth::AuthManager;
use crate::session::shared_context_manager::SharedContextManager;
use crate::types::SessionInfo;
use crate::utils::page_utils::{snapshot_all_responses, wait_for_latest_answer, WaitOptions};
use crate::utils::stealth::{
    char_type_delay_ms, effective_wpm, random_delay, wpm_to_avg_char_delay_ms,
};

// ---------------------------------------------------------------------------
// Chat input selectors (highest priority first, matching Python/TS)
// ---------------------------------------------------------------------------

const CHAT_INPUT_SELECTORS: &[&str] = &[
    "textarea.query-box-input",
    r#"textarea[aria-label="Feld für Anfragen"]"#,
];

// ---------------------------------------------------------------------------
// BrowserSession
// ---------------------------------------------------------------------------

pub struct BrowserSession {
    pub session_id: String,
    pub notebook_url: String,
    pub created_at: Instant,
    last_activity: std::sync::Mutex<Instant>,
    pub message_count: AtomicUsize,
    page: Mutex<Option<Page>>,
    initialized: AtomicBool,
    shared_ctx: Arc<SharedContextManager>,
    auth: Arc<AuthManager>,
}

impl BrowserSession {
    pub fn new(
        session_id: String,
        notebook_url: String,
        shared_ctx: Arc<SharedContextManager>,
        auth: Arc<AuthManager>,
    ) -> Self {
        tracing::info!("BrowserSession {} created ({})", session_id, notebook_url);
        Self {
            session_id,
            notebook_url,
            created_at: Instant::now(),
            last_activity: std::sync::Mutex::new(Instant::now()),
            message_count: AtomicUsize::new(0),
            page: Mutex::new(None),
            initialized: AtomicBool::new(false),
            shared_ctx,
            auth,
        }
    }

    // -----------------------------------------------------------------------
    // Public: init
    // -----------------------------------------------------------------------

    /// Navigate to the notebook, check/inject cookies, wait for the UI to load.
    pub async fn init(&self) -> Result<()> {
        if self.initialized.load(Ordering::SeqCst) {
            return Ok(());
        }

        tracing::info!("Initializing session {}...", self.session_id);

        // Get a new blank tab from the shared browser
        let page = self.shared_ctx.new_page(None).await?;

        // Navigate to the notebook
        tracing::info!("  Navigating to: {}", self.notebook_url);
        page.goto(&self.notebook_url)
            .await
            .map_err(|e| anyhow!("Navigation failed: {e}"))?;

        random_delay(2000.0, 3000.0).await;

        // Check / restore auth
        let is_auth = self.auth.validate_cookies_expiry(&page).await;
        if !is_auth {
            if self.auth.has_saved_state() {
                tracing::info!("  Loading saved auth state...");
                self.auth
                    .load_auth_state(&page)
                    .await
                    .map_err(|e| anyhow!("Auth state load failed: {e}"))?;
                // Reload page to apply cookies
                page.goto(&self.notebook_url)
                    .await
                    .map_err(|e| anyhow!("Reload after auth failed: {e}"))?;
                random_delay(2000.0, 3000.0).await;
            } else {
                return Err(anyhow!(
                    "Not authenticated. Please run setup_auth or provide \
                    NOTEBOOKLM_EMAIL and NOTEBOOKLM_PASSWORD."
                ));
            }
        }

        // Restore session storage (best-effort — errors are logged, not propagated)
        if let Err(e) = self.auth.load_session_storage(&page).await {
            tracing::warn!("  load_session_storage (non-fatal): {e}");
        }

        // Wait for the NotebookLM textarea to appear
        tracing::info!("  Waiting for NotebookLM interface...");
        wait_for_input_ready(&page).await?;

        // Store the ready page
        *self.page.lock().await = Some(page);
        self.initialized.store(true, Ordering::SeqCst);
        self.update_activity();

        tracing::info!("Session {} initialized successfully", self.session_id);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Public: ask
    // -----------------------------------------------------------------------

    /// Ask a question, with automatic session recovery on closed-browser errors.
    pub async fn ask(&self, question: &str) -> Result<String> {
        match self.ask_once(question).await {
            Err(e) if is_closed_error(&e) => {
                tracing::warn!(
                    "Closed page/context on session {} — recovering...",
                    self.session_id
                );
                self.initialized.store(false, Ordering::SeqCst);
                *self.page.lock().await = None;
                self.init().await?;
                self.ask_once(question).await
            }
            other => other,
        }
    }

    async fn ask_once(&self, question: &str) -> Result<String> {
        if !self.initialized.load(Ordering::SeqCst) {
            self.init().await?;
        }

        // Hold the page lock for the entire ask duration (serialises concurrent asks)
        let page_guard = self.page.lock().await;
        let page = page_guard
            .as_ref()
            .ok_or_else(|| anyhow!("Page not initialized"))?;

        tracing::info!(
            "[{}] Asking: \"{}...\"",
            self.session_id,
            &question[..question.len().min(80)]
        );

        // Snapshot existing responses BEFORE asking
        let existing = snapshot_all_responses(page).await;
        tracing::debug!("  Snapshotted {} existing response(s)", existing.len());

        // Type and submit the question
        human_type_into_chat(page, question).await?;
        random_delay(500.0, 1000.0).await;

        // Press Enter to submit
        let input_el = find_first_element(page, CHAT_INPUT_SELECTORS)
            .await
            .ok_or_else(|| anyhow!("Chat input not found for Enter key press"))?;
        input_el
            .press_key("Enter")
            .await
            .map_err(|e| anyhow!("press_key(Enter) failed: {e}"))?;

        random_delay(1000.0, 1500.0).await;

        // Wait for the new response (streaming detection, 2 min timeout)
        tracing::info!("  Waiting for NotebookLM response...");
        let opts = WaitOptions {
            question: question.to_string(),
            ignore_texts: existing,
            ..WaitOptions::default()
        };
        let answer = wait_for_latest_answer(page, opts)
            .await
            .ok_or_else(|| anyhow!("Timeout: NotebookLM did not respond within 2 minutes"))?;

        // Update session stats
        self.message_count.fetch_add(1, Ordering::Relaxed);
        self.update_activity();

        tracing::info!(
            "[{}] Answer received ({} chars, {} total messages)",
            self.session_id,
            answer.len(),
            self.message_count.load(Ordering::Relaxed)
        );

        Ok(format!(
            "{answer}{}",
            crate::tools::handlers::follow_up_reminder()
        ))
    }

    // -----------------------------------------------------------------------
    // Public: reset
    // -----------------------------------------------------------------------

    /// Reset the chat by reloading the notebook page (clears conversation history).
    pub async fn reset(&self) -> Result<()> {
        match self.reset_once().await {
            Err(e) if is_closed_error(&e) => {
                tracing::warn!(
                    "Closed context during reset of {} — recovering...",
                    self.session_id
                );
                self.initialized.store(false, Ordering::SeqCst);
                *self.page.lock().await = None;
                self.init().await
            }
            other => other,
        }
    }

    async fn reset_once(&self) -> Result<()> {
        if !self.initialized.load(Ordering::SeqCst) {
            return self.init().await;
        }
        tracing::info!("Resetting chat history for session {}...", self.session_id);

        let page_guard = self.page.lock().await;
        let page = page_guard
            .as_ref()
            .ok_or_else(|| anyhow!("Page not initialized"))?;

        page.goto(&self.notebook_url)
            .await
            .map_err(|e| anyhow!("Reset navigation failed: {e}"))?;
        random_delay(2000.0, 3000.0).await;
        wait_for_input_ready(page).await?;

        self.message_count.store(0, Ordering::Relaxed);
        self.update_activity();
        tracing::info!("Session {} reset", self.session_id);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Public: close
    // -----------------------------------------------------------------------

    pub async fn close(&self) {
        let mut guard = self.page.lock().await;
        if let Some(page) = guard.take() {
            if let Err(e) = page.close().await {
                tracing::warn!("Error closing page for {}: {e}", self.session_id);
            }
        }
        self.initialized.store(false, Ordering::SeqCst);
        tracing::info!("Session {} closed", self.session_id);
    }

    // -----------------------------------------------------------------------
    // Public: helpers
    // -----------------------------------------------------------------------

    pub fn is_expired(&self, timeout_seconds: u64) -> bool {
        self.last_activity
            .lock()
            .unwrap()
            .elapsed()
            .as_secs()
            > timeout_seconds
    }

    pub fn update_activity(&self) {
        *self.last_activity.lock().unwrap() = Instant::now();
    }

    pub fn get_info(&self) -> SessionInfo {
        use std::time::SystemTime;
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let age_ms = self.created_at.elapsed().as_millis() as i64;
        let inactive_secs = self.last_activity.lock().unwrap().elapsed().as_secs();

        SessionInfo {
            id: self.session_id.clone(),
            created_at: now_ms - age_ms,
            last_activity: now_ms - (inactive_secs as i64 * 1000),
            age_seconds: self.created_at.elapsed().as_secs(),
            inactive_seconds: inactive_secs,
            message_count: self.message_count.load(Ordering::Relaxed),
            notebook_url: self.notebook_url.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Poll for any of the chat input selectors to appear (up to 10 seconds).
async fn wait_for_input_ready(page: &Page) -> Result<()> {
    let timeout = Duration::from_secs(10);
    let start = Instant::now();

    while start.elapsed() < timeout {
        for selector in CHAT_INPUT_SELECTORS {
            if page.find_element(*selector).await.is_ok() {
                tracing::info!("  Chat input ready ({})", selector);
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Err(anyhow!(
        "Timeout: NotebookLM chat input did not appear within 10 seconds. \
        Check that the notebook URL is correct and you are authenticated."
    ))
}

/// Find the first element that matches any of the given selectors.
async fn find_first_element(
    page: &Page,
    selectors: &[&str],
) -> Option<chromiumoxide::Element> {
    for selector in selectors {
        if let Ok(el) = page.find_element(*selector).await {
            return Some(el);
        }
    }
    None
}

/// Type text into the chat input with human-like per-character delays.
async fn human_type_into_chat(page: &Page, text: &str) -> Result<()> {
    let el = find_first_element(page, CHAT_INPUT_SELECTORS)
        .await
        .ok_or_else(|| anyhow!("Could not find chat input element for typing"))?;

    // Click to focus the textarea
    el.click()
        .await
        .map_err(|e| anyhow!("Click on chat input failed: {e}"))?;
    random_delay(200.0, 400.0).await;

    // Type each character with a Gaussian-distributed inter-key delay
    let wpm = effective_wpm(None);
    let avg_delay_ms = wpm_to_avg_char_delay_ms(wpm);

    for ch in text.chars() {
        let ch_str = ch.to_string();
        el.type_str(&ch_str)
            .await
            .map_err(|e| anyhow!("type_str failed for char '{}': {e}", ch))?;

        let delay_ms = char_type_delay_ms(ch, avg_delay_ms);
        if delay_ms >= 1.0 {
            tokio::time::sleep(Duration::from_millis(delay_ms as u64)).await;
        }
    }

    Ok(())
}

/// Check if an error message indicates the browser page/context was closed.
fn is_closed_error(e: &anyhow::Error) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("has been closed")
        || msg.contains("target closed")
        || msg.contains("browser has been closed")
        || msg.contains("context closed")
        || msg.contains("session closed")
}
