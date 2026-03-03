//! Session Manager
//! (Rust rewrite of src/session/session-manager.ts)
//!
//! Maintains a pool of `BrowserSession` instances (one per session ID).
//! All sessions share the ONE persistent Chrome profile via `SharedContextManager`.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use dashmap::DashMap;
use rand::Rng;

use crate::auth::AuthManager;
use crate::config::config;
use crate::session::browser_session::BrowserSession;
use crate::session::shared_context_manager::SharedContextManager;
use crate::types::SessionInfo;

// ---------------------------------------------------------------------------
// SessionManager
// ---------------------------------------------------------------------------

pub struct SessionManager {
    sessions: DashMap<String, Arc<BrowserSession>>,
    shared_ctx: Arc<SharedContextManager>,
    auth: Arc<AuthManager>,
}

impl SessionManager {
    pub fn new(auth: Arc<AuthManager>) -> Self {
        let shared_ctx = Arc::new(SharedContextManager::new(Arc::clone(&auth)));
        let cfg = config();
        tracing::info!("SessionManager initialized");
        tracing::info!("  Max sessions: {}", cfg.max_sessions);
        tracing::info!(
            "  Session timeout: {}s ({}m)",
            cfg.session_timeout,
            cfg.session_timeout / 60
        );
        Self {
            sessions: DashMap::new(),
            shared_ctx,
            auth,
        }
    }

    // -----------------------------------------------------------------------
    // Core: get or create session
    // -----------------------------------------------------------------------

    /// Return an existing session or create a new one.
    ///
    /// * `session_id` – optional ID to reuse; auto-generated if `None`
    /// * `notebook_url` – required if creating a new session; falls back to
    ///   `NOTEBOOK_URL` env var
    /// * `show_browser` – override headless mode (`Some(true)` = visible)
    pub async fn get_or_create_session(
        &self,
        session_id: Option<String>,
        notebook_url: Option<String>,
        _show_browser: Option<bool>,
    ) -> Result<Arc<BrowserSession>> {
        let cfg = config();

        // Resolve notebook URL
        let target_url = notebook_url
            .filter(|u| !u.trim().is_empty())
            .or_else(|| {
                if cfg.notebook_url.is_empty() {
                    None
                } else {
                    Some(cfg.notebook_url.clone())
                }
            })
            .ok_or_else(|| {
                anyhow!(
                    "Notebook URL is required. Provide it via the 'notebook_url' parameter \
                    or set the NOTEBOOK_URL environment variable."
                )
            })?;

        if !target_url.starts_with("http") {
            return Err(anyhow!(
                "Notebook URL must be an absolute URL (starting with http)"
            ));
        }

        // Generate or use provided session ID
        let sid = session_id.unwrap_or_else(|| {
            let bytes: [u8; 4] = rand::thread_rng().gen();
            let id = hex::encode(bytes);
            tracing::info!("Auto-generated session ID: {id}");
            id
        });

        // Return existing session if URL matches
        if let Some(entry) = self.sessions.get(&sid) {
            let session = Arc::clone(entry.value());
            if session.notebook_url == target_url {
                session.update_activity();
                tracing::info!("Reusing session {sid}");
                return Ok(session);
            }
            // URL changed — close old session and recreate
            drop(entry); // release read guard before async close
            tracing::info!("Session {sid} URL changed — recreating...");
            if let Some((_, old)) = self.sessions.remove(&sid) {
                old.close().await;
            }
        }

        // Enforce max session limit (evict oldest if needed)
        if self.sessions.len() >= cfg.max_sessions as usize {
            tracing::warn!(
                "Max sessions ({}) reached — evicting oldest",
                cfg.max_sessions
            );
            self.evict_oldest().await;
        }

        // Create and initialise new session
        tracing::info!("Creating new session {sid} for {target_url}...");
        let session = Arc::new(BrowserSession::new(
            sid.clone(),
            target_url,
            Arc::clone(&self.shared_ctx),
            Arc::clone(&self.auth),
        ));
        session.init().await?;
        self.sessions.insert(sid.clone(), Arc::clone(&session));
        tracing::info!(
            "Session {sid} created ({}/{} active)",
            self.sessions.len(),
            cfg.max_sessions
        );
        Ok(session)
    }

    // -----------------------------------------------------------------------
    // Session lookup
    // -----------------------------------------------------------------------

    pub fn get_session(&self, session_id: &str) -> Option<Arc<BrowserSession>> {
        self.sessions.get(session_id).map(|e| Arc::clone(e.value()))
    }

    // -----------------------------------------------------------------------
    // Session closing
    // -----------------------------------------------------------------------

    /// Close a specific session. Returns `true` if the session was found.
    pub async fn close_session(&self, session_id: &str) -> bool {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            session.close().await;
            tracing::info!(
                "Session {session_id} closed ({}/{} active)",
                self.sessions.len(),
                config().max_sessions
            );
            true
        } else {
            false
        }
    }

    /// Close all sessions and the shared browser context.
    pub async fn close_all(&self) {
        let ids: Vec<String> = self.sessions.iter().map(|e| e.key().clone()).collect();
        if !ids.is_empty() {
            tracing::info!("Closing all {} sessions...", ids.len());
        }
        for id in ids {
            if let Some((_, session)) = self.sessions.remove(&id) {
                session.close().await;
            }
        }
        self.shared_ctx.close().await;
        tracing::info!("All sessions closed");
    }

    // -----------------------------------------------------------------------
    // Cleanup: remove expired sessions
    // -----------------------------------------------------------------------

    pub async fn cleanup_expired(&self) -> usize {
        let cfg = config();
        let expired: Vec<String> = self
            .sessions
            .iter()
            .filter(|e| e.value().is_expired(cfg.session_timeout))
            .map(|e| e.key().clone())
            .collect();

        for id in &expired {
            if let Some((_, session)) = self.sessions.remove(id) {
                session.close().await;
            }
        }

        if !expired.is_empty() {
            tracing::info!("Cleaned up {} expired session(s)", expired.len());
        }
        expired.len()
    }

    // -----------------------------------------------------------------------
    // Info
    // -----------------------------------------------------------------------

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|e| e.value().get_info())
            .collect()
    }

    pub fn get_stats(&self) -> serde_json::Value {
        let cfg = config();
        let sessions = self.list_sessions();
        let total_messages: usize = sessions.iter().map(|s| s.message_count).sum();
        let oldest_age = sessions.iter().map(|s| s.age_seconds).max().unwrap_or(0);
        serde_json::json!({
            "active_sessions": sessions.len(),
            "max_sessions": cfg.max_sessions,
            "session_timeout_seconds": cfg.session_timeout,
            "oldest_session_seconds": oldest_age,
            "total_messages": total_messages
        })
    }

    // -----------------------------------------------------------------------
    // Private
    // -----------------------------------------------------------------------

    async fn evict_oldest(&self) {
        // Find the session with the earliest `created_at` (oldest first)
        let oldest_id = self
            .sessions
            .iter()
            .min_by_key(|e| e.value().created_at)
            .map(|e| e.key().clone());

        if let Some(id) = oldest_id {
            if let Some((_, session)) = self.sessions.remove(&id) {
                session.close().await;
                tracing::info!("Evicted oldest session {id}");
            }
        }
    }
}
