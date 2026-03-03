//! MCP JSON-RPC 2.0 server over stdio
//!
//! Implements the Model Context Protocol transport layer manually, avoiding
//! external SDK version churn. Messages are newline-delimited JSON on stdio.
//!
//! Protocol reference: https://spec.modelcontextprotocol.io/

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use notebooklm_core::{
    auth::AuthManager,
    library::NotebookLibrary,
    resources::ResourceHandlers,
    session::SessionManager,
    tools::{definitions::build_all_tools, ToolHandlers},
    utils::settings_manager::SettingsManager,
};

// ---------------------------------------------------------------------------
// McpServer
// ---------------------------------------------------------------------------

pub struct McpServer {
    tool_handlers: ToolHandlers,
    resource_handlers: ResourceHandlers,
    settings: Arc<SettingsManager>,
    started_at: Instant,
}

impl McpServer {
    pub fn new() -> Result<Self> {
        let library = Arc::new(NotebookLibrary::new()?);
        let settings = Arc::new(SettingsManager::new());
        let auth = Arc::new(AuthManager::new());
        let session_manager = Arc::new(SessionManager::new(Arc::clone(&auth)));

        Ok(Self {
            tool_handlers: ToolHandlers::new(Arc::clone(&library), session_manager, auth),
            resource_handlers: ResourceHandlers::new(Arc::clone(&library)),
            settings,
            started_at: Instant::now(),
        })
    }

    /// Run the JSON-RPC event loop until stdin closes (client disconnects).
    pub async fn serve(self) -> Result<()> {
        tracing::info!("MCP server listening on stdio");

        let mut stdin = BufReader::new(tokio::io::stdin());
        let mut stdout = tokio::io::stdout();
        let mut line = String::new();

        loop {
            line.clear();
            match stdin.read_line(&mut line).await {
                Ok(0) => {
                    tracing::info!("stdin closed — shutting down");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("stdin read error: {e}");
                    break;
                }
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            tracing::debug!("← {trimmed}");

            let response = self.process_message(trimmed).await;

            if let Some(resp) = response {
                let out = serde_json::to_string(&resp).unwrap_or_default() + "\n";
                tracing::debug!("→ {}", out.trim());
                if let Err(e) = stdout.write_all(out.as_bytes()).await {
                    tracing::error!("stdout write error: {e}");
                    break;
                }
                if let Err(e) = stdout.flush().await {
                    tracing::error!("stdout flush error: {e}");
                    break;
                }
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Message dispatch
    // -----------------------------------------------------------------------

    async fn process_message(&self, raw: &str) -> Option<Value> {
        let msg: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => {
                return Some(rpc_error(Value::Null, -32700, "Parse error", Some(e.to_string())));
            }
        };

        // Notifications have no "id" field — process but do not respond
        let id = match msg.get("id") {
            Some(id) => id.clone(),
            None => {
                let method = msg["method"].as_str().unwrap_or("");
                tracing::debug!("[notification] {method}");
                return None;
            }
        };

        let method = msg["method"].as_str().unwrap_or("");
        let params = msg.get("params").unwrap_or(&Value::Null);

        tracing::info!("[MCP] {method}");

        let result = match method {
            "initialize" => Ok(self.handle_initialize(params)),
            "tools/list" => Ok(self.handle_list_tools()),
            "tools/call" => self.handle_call_tool(params).await,
            "resources/list" => Ok(self.resource_handlers.handle_list_resources()),
            "resources/read" => self
                .resource_handlers
                .handle_read_resource(params)
                .map_err(|e| e.to_string()),
            "resources/templates/list" => {
                Ok(self.resource_handlers.handle_list_resource_templates())
            }
            "completion/complete" => Ok(self.resource_handlers.handle_complete(params)),
            "ping" => Ok(json!({})),
            _ => Err(format!("Method not found: {method}")),
        };

        Some(match result {
            Ok(r) => rpc_success(id, r),
            Err(e) => rpc_error(id, -32603, "Internal error", Some(e)),
        })
    }

    // -----------------------------------------------------------------------
    // initialize
    // -----------------------------------------------------------------------

    fn handle_initialize(&self, params: &Value) -> Value {
        let client_name = params["clientInfo"]["name"].as_str().unwrap_or("unknown");
        let client_version = params["clientInfo"]["version"].as_str().unwrap_or("?");
        tracing::info!("Client: {client_name} v{client_version}");

        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {},
                "resources": {
                    "subscribe": false,
                    "listChanged": false
                },
                "logging": {},
                "completions": {}
            },
            "serverInfo": {
                "name": "notebooklm-mcp",
                "version": env!("CARGO_PKG_VERSION")
            }
        })
    }

    // -----------------------------------------------------------------------
    // tools/list
    // -----------------------------------------------------------------------

    fn handle_list_tools(&self) -> Value {
        let all_tools = build_all_tools_filtered(&self.tool_handlers, &self.settings);
        tracing::info!("tools/list → {} tools", all_tools.len());
        json!({ "tools": all_tools })
    }

    // -----------------------------------------------------------------------
    // tools/call
    // -----------------------------------------------------------------------

    async fn handle_call_tool(&self, params: &Value) -> Result<Value, String> {
        let name = params["name"]
            .as_str()
            .ok_or("Missing tool name in tools/call")?;
        let args = params.get("arguments").unwrap_or(&Value::Null);

        tracing::info!("[MCP] Tool: {name}");

        let uptime = self.started_at.elapsed().as_secs();

        let result: anyhow::Result<Value> = match name {
            "ask_question" => self.tool_handlers.handle_ask_question(args).await,
            "add_notebook" => self.tool_handlers.handle_add_notebook(args).await,
            "list_notebooks" => self.tool_handlers.handle_list_notebooks().await,
            "get_notebook" => self.tool_handlers.handle_get_notebook(args).await,
            "select_notebook" => self.tool_handlers.handle_select_notebook(args).await,
            "update_notebook" => self.tool_handlers.handle_update_notebook(args).await,
            "remove_notebook" => self.tool_handlers.handle_remove_notebook(args).await,
            "search_notebooks" => self.tool_handlers.handle_search_notebooks(args).await,
            "get_library_stats" => self.tool_handlers.handle_get_library_stats().await,
            "list_sessions" => self.tool_handlers.handle_list_sessions().await,
            "close_session" => self.tool_handlers.handle_close_session(args).await,
            "reset_session" => self.tool_handlers.handle_reset_session(args).await,
            "get_health" => self.tool_handlers.handle_get_health(uptime).await,
            "setup_auth" => self.tool_handlers.handle_setup_auth(args).await,
            "re_auth" => self.tool_handlers.handle_re_auth(args).await,
            "cleanup_data" => self.tool_handlers.handle_cleanup_data(args).await,
            _ => {
                let msg = format!("Unknown tool: {name}");
                return Ok(tool_call_result(
                    &json!({"success": false, "error": msg}),
                    true,
                ));
            }
        };

        match result {
            Ok(r) => Ok(tool_call_result(&r, false)),
            Err(e) => {
                tracing::error!("Tool '{name}' error: {e}");
                Ok(tool_call_result(
                    &json!({"success": false, "error": e.to_string()}),
                    true,
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: build tool list with profile filtering applied
// ---------------------------------------------------------------------------

fn build_all_tools_filtered(
    _handlers: &ToolHandlers,
    settings: &SettingsManager,
) -> Vec<Value> {
    let library = match NotebookLibrary::new() {
        Ok(lib) => lib,
        Err(e) => {
            tracing::warn!("Could not load library for tools/list: {e}");
            return vec![];
        }
    };

    let all_tools = build_all_tools(&library);

    // Collect names first (owned strings) to avoid borrow-after-move
    let names_owned: Vec<String> = all_tools
        .iter()
        .filter_map(|t| t["name"].as_str().map(|s| s.to_owned()))
        .collect();
    let name_refs: Vec<&str> = names_owned.iter().map(String::as_str).collect();
    let allowed: HashSet<&str> = settings.filter_tool_names(&name_refs).into_iter().collect();

    all_tools
        .into_iter()
        .filter(|t| t["name"].as_str().map(|n| allowed.contains(n)).unwrap_or(false))
        .collect()
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

fn rpc_success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i32, message: &str, data: Option<String>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(d) = data {
        error["data"] = json!(d);
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// Wrap a tool result value into the MCP `CallToolResult` format.
fn tool_call_result(result: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(result).unwrap_or_default();
    let mut out = json!({
        "content": [{ "type": "text", "text": text }]
    });
    if is_error {
        out["isError"] = json!(true);
    }
    out
}
