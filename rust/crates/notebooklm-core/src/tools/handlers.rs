//! MCP tool handler implementations (Rust rewrite of src/tools/handlers.ts)
//!
//! Phase 5: Browser tools are fully wired (ask_question, session management,
//!          get_health). Auth tools and cleanup remain stubs (Phase 6/8).

use std::sync::Arc;

use anyhow::anyhow;
use serde_json::{json, Value};

use crate::auth::AuthManager;
use crate::library::{AddNotebookInput, NotebookLibrary, UpdateNotebookInput};
use crate::session::SessionManager;

/// Follow-up reminder appended to every successful `ask_question` answer.
const FOLLOW_UP_REMINDER: &str = "\n\n---\n\
💡 **Keep researching?** If this answer is incomplete or you need more details, \
ask a follow-up question in the same session (`session_id`). \
Great answers often require 2–4 targeted questions. \
Only reply to the user once you are **100% sure** the information is complete.";

// ---------------------------------------------------------------------------
// ToolHandlers
// ---------------------------------------------------------------------------

pub struct ToolHandlers {
    library: Arc<NotebookLibrary>,
    session_manager: Arc<SessionManager>,
    auth_manager: Arc<AuthManager>,
}

impl ToolHandlers {
    pub fn new(
        library: Arc<NotebookLibrary>,
        session_manager: Arc<SessionManager>,
        auth_manager: Arc<AuthManager>,
    ) -> Self {
        Self {
            library,
            session_manager,
            auth_manager,
        }
    }

    // -----------------------------------------------------------------------
    // ask_question  (Phase 5)
    // -----------------------------------------------------------------------

    pub async fn handle_ask_question(&self, args: &Value) -> anyhow::Result<Value> {
        let question = args["question"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing required parameter 'question'"))?
            .to_string();

        // Notebook URL: argument → library active notebook → NOTEBOOK_URL env
        let notebook_url =
            if let Some(u) = args["notebook_url"].as_str().filter(|s| !s.is_empty()) {
                Some(u.to_string())
            } else {
                self.library
                    .get_active_notebook()
                    .map(|nb| nb.url.clone())
            };

        let session_id = args["session_id"].as_str().map(|s| s.to_string());
        let show_browser = args["show_browser"].as_bool();

        tracing::info!(
            "ask_question: session={:?} url={:?} q=\"{}...\"",
            session_id,
            notebook_url
                .as_deref()
                .map(|u| &u[..u.len().min(50)])
                .unwrap_or("(none)"),
            &question[..question.len().min(80)]
        );

        let nb_url = notebook_url.ok_or_else(|| {
            anyhow!(
                "No notebook selected. Use add_notebook + select_notebook first, \
                or pass notebook_url directly to ask_question."
            )
        })?;

        // Get or create the browser session
        let session = self
            .session_manager
            .get_or_create_session(session_id, Some(nb_url.clone()), show_browser)
            .await
            .map_err(|e| anyhow!("Failed to create browser session: {e}"))?;

        // Increment notebook use count (best effort)
        if let Some(active) = self.library.get_active_notebook() {
            let _ = self.library.increment_use_count(&active.id);
        }

        // Ask the question
        let answer = session
            .ask(&question)
            .await
            .map_err(|e| anyhow!("ask_question failed: {e}"))?;

        let info = session.get_info();
        Ok(json!({
            "success": true,
            "status": "success",
            "question": question,
            "answer": answer,
            "notebook_url": nb_url,
            "session_id": session.session_id,
            "session_info": {
                "age_seconds": info.age_seconds,
                "message_count": info.message_count,
                "last_activity": info.last_activity
            }
        }))
    }

    // -----------------------------------------------------------------------
    // Notebook library tools  (fully implemented)
    // -----------------------------------------------------------------------

    pub async fn handle_add_notebook(&self, args: &Value) -> anyhow::Result<Value> {
        let input: AddNotebookInput = serde_json::from_value(args.clone())
            .map_err(|e| anyhow!("Invalid add_notebook parameters: {e}"))?;

        let notebook = self.library.add_notebook(input)?;
        Ok(json!({
            "success": true,
            "data": {
                "notebook": notebook,
                "message": format!("Notebook '{}' added to library (id: {})", notebook.name, notebook.id)
            }
        }))
    }

    pub async fn handle_list_notebooks(&self) -> anyhow::Result<Value> {
        let notebooks = self.library.list_notebooks();
        let active = self.library.get_active_notebook();
        Ok(json!({
            "success": true,
            "data": {
                "notebooks": notebooks,
                "active_notebook_id": active.as_ref().map(|n| &n.id),
                "total": notebooks.len()
            }
        }))
    }

    pub async fn handle_get_notebook(&self, args: &Value) -> anyhow::Result<Value> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing required parameter 'id'"))?;
        match self.library.get_notebook(id) {
            Some(notebook) => Ok(json!({ "success": true, "data": { "notebook": notebook } })),
            None => Ok(json!({ "success": false, "error": format!("Notebook not found: {id}") })),
        }
    }

    pub async fn handle_select_notebook(&self, args: &Value) -> anyhow::Result<Value> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing required parameter 'id'"))?;
        match self.library.select_notebook(id) {
            Ok(notebook) => Ok(json!({
                "success": true,
                "data": {
                    "notebook": notebook,
                    "message": format!("Active notebook set to '{}'", notebook.name)
                }
            })),
            Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
        }
    }

    pub async fn handle_update_notebook(&self, args: &Value) -> anyhow::Result<Value> {
        let input: UpdateNotebookInput = serde_json::from_value(args.clone())
            .map_err(|e| anyhow!("Invalid update_notebook parameters: {e}"))?;
        match self.library.update_notebook(input) {
            Ok(notebook) => Ok(json!({
                "success": true,
                "data": {
                    "notebook": notebook,
                    "message": format!("Notebook '{}' updated", notebook.id)
                }
            })),
            Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
        }
    }

    pub async fn handle_remove_notebook(&self, args: &Value) -> anyhow::Result<Value> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing required parameter 'id'"))?;
        match self.library.remove_notebook(id) {
            Ok(true) => Ok(json!({
                "success": true,
                "data": {
                    "removed_id": id,
                    "message": format!("Notebook '{id}' removed from library")
                }
            })),
            Ok(false) => {
                Ok(json!({ "success": false, "error": format!("Notebook not found: {id}") }))
            }
            Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
        }
    }

    pub async fn handle_search_notebooks(&self, args: &Value) -> anyhow::Result<Value> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing required parameter 'query'"))?;
        let results = self.library.search_notebooks(query);
        Ok(json!({
            "success": true,
            "data": {
                "results": results,
                "query": query,
                "count": results.len()
            }
        }))
    }

    pub async fn handle_get_library_stats(&self) -> anyhow::Result<Value> {
        let stats = self.library.get_stats();
        Ok(json!({ "success": true, "data": stats }))
    }

    // -----------------------------------------------------------------------
    // Session management  (Phase 5)
    // -----------------------------------------------------------------------

    pub async fn handle_list_sessions(&self) -> anyhow::Result<Value> {
        let sessions = self.session_manager.list_sessions();
        let count = sessions.len();
        Ok(json!({
            "success": true,
            "data": {
                "sessions": sessions,
                "count": count
            }
        }))
    }

    pub async fn handle_close_session(&self, args: &Value) -> anyhow::Result<Value> {
        let session_id = args["session_id"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing required parameter 'session_id'"))?;
        let closed = self.session_manager.close_session(session_id).await;
        if closed {
            Ok(json!({
                "success": true,
                "data": { "message": format!("Session '{session_id}' closed") }
            }))
        } else {
            Ok(json!({
                "success": false,
                "error": format!("Session not found: {session_id}")
            }))
        }
    }

    pub async fn handle_reset_session(&self, args: &Value) -> anyhow::Result<Value> {
        let session_id = args["session_id"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing required parameter 'session_id'"))?;
        match self.session_manager.get_session(session_id) {
            Some(session) => {
                session
                    .reset()
                    .await
                    .map_err(|e| anyhow!("Reset failed: {e}"))?;
                Ok(json!({
                    "success": true,
                    "data": {
                        "message": format!("Session '{session_id}' chat history reset"),
                        "session_id": session_id
                    }
                }))
            }
            None => Ok(json!({
                "success": false,
                "error": format!("Session not found: {session_id}")
            })),
        }
    }

    // -----------------------------------------------------------------------
    // get_health  (Phase 5)
    // -----------------------------------------------------------------------

    pub async fn handle_get_health(&self, uptime_seconds: u64) -> anyhow::Result<Value> {
        let cfg = crate::config::config();
        let stats = self.library.get_stats();
        let session_stats = self.session_manager.get_stats();
        let auth_info = self.auth_manager.get_auth_info();
        let settings = crate::utils::settings_manager::SettingsManager::new();
        let effective = settings.get_effective_settings();

        Ok(json!({
            "success": true,
            "data": {
                "server": {
                    "version": env!("CARGO_PKG_VERSION"),
                    "runtime": "Rust (Phase 5 — browser automation active)",
                    "uptime_seconds": uptime_seconds,
                    "phase": "Phase 5 — browser automation active"
                },
                "authentication": auth_info,
                "sessions": session_stats,
                "config": {
                    "headless": cfg.headless,
                    "max_sessions": cfg.max_sessions,
                    "session_timeout_seconds": cfg.session_timeout,
                    "stealth_enabled": cfg.stealth_enabled,
                    "data_dir": cfg.data_dir.display().to_string(),
                    "config_dir": cfg.config_dir.display().to_string()
                },
                "library": {
                    "total_notebooks": stats.total_notebooks,
                    "active_notebook": stats.active_notebook,
                    "total_queries": stats.total_queries,
                    "last_modified": stats.last_modified
                },
                "profile": effective.profile
            }
        }))
    }

    // -----------------------------------------------------------------------
    // Auth tools  (Phase 6 stubs)
    // -----------------------------------------------------------------------

    pub async fn handle_setup_auth(&self, _args: &Value) -> anyhow::Result<Value> {
        Ok(json!({
            "success": false,
            "error": "Interactive setup_auth requires Phase 6 of the Rust migration. \
Use the TypeScript version (`npx notebooklm-mcp`) for authentication setup, \
or set NOTEBOOKLM_EMAIL and NOTEBOOKLM_PASSWORD environment variables."
        }))
    }

    pub async fn handle_re_auth(&self, _args: &Value) -> anyhow::Result<Value> {
        Ok(json!({
            "success": false,
            "error": "Interactive re_auth requires Phase 6 of the Rust migration. \
Use the TypeScript version (`npx notebooklm-mcp`) for re-authentication."
        }))
    }

    // -----------------------------------------------------------------------
    // cleanup_data  (Phase 8 stub)
    // -----------------------------------------------------------------------

    pub async fn handle_cleanup_data(&self, _args: &Value) -> anyhow::Result<Value> {
        Ok(json!({
            "success": false,
            "error": "cleanup_data not yet implemented (Phase 8 of Rust migration)."
        }))
    }
}

// ---------------------------------------------------------------------------
// Public constant for browser_session.rs
// ---------------------------------------------------------------------------

pub fn follow_up_reminder() -> &'static str {
    FOLLOW_UP_REMINDER
}
