//! MCP resource + completion handlers (Rust rewrite of src/resources/resource-handlers.ts)

use std::sync::Arc;

use anyhow::anyhow;
use serde_json::{json, Value};

use crate::library::NotebookLibrary;

pub struct ResourceHandlers {
    library: Arc<NotebookLibrary>,
}

impl ResourceHandlers {
    pub fn new(library: Arc<NotebookLibrary>) -> Self {
        Self { library }
    }

    // -----------------------------------------------------------------------
    // resources/list
    // -----------------------------------------------------------------------

    pub fn handle_list_resources(&self) -> Value {
        let notebooks = self.library.list_notebooks();
        let active = self.library.get_active_notebook();

        let mut resources = vec![json!({
            "uri": "notebooklm://library",
            "name": "Notebook Library",
            "description": "Complete notebook library with all available knowledge sources. \
Read this to discover what notebooks are available. \
⚠️ If you think a notebook might help with the user's task, \
ASK THE USER FOR PERMISSION before consulting it: \
'Should I consult the [notebook] for this task?'",
            "mimeType": "application/json"
        })];

        for nb in &notebooks {
            resources.push(json!({
                "uri": format!("notebooklm://library/{}", nb.id),
                "name": nb.name,
                "description": format!(
                    "{} | Topics: {} | 💡 Use ask_question to query this notebook",
                    nb.description,
                    nb.topics.join(", ")
                ),
                "mimeType": "application/json"
            }));
        }

        // Legacy metadata resource (backwards compatibility)
        if active.is_some() {
            resources.push(json!({
                "uri": "notebooklm://metadata",
                "name": "Active Notebook Metadata (Legacy)",
                "description": "Information about the currently active notebook. \
DEPRECATED: Use notebooklm://library instead for multi-notebook support. \
⚠️ Always ask user permission before using notebooks for tasks they didn't explicitly mention.",
                "mimeType": "application/json"
            }));
        }

        json!({ "resources": resources })
    }

    // -----------------------------------------------------------------------
    // resources/templates/list
    // -----------------------------------------------------------------------

    pub fn handle_list_resource_templates(&self) -> Value {
        json!({
            "resourceTemplates": [{
                "uriTemplate": "notebooklm://library/{id}",
                "name": "Notebook by ID",
                "description": "Access a specific notebook from your library by ID. \
Provides detailed metadata including topics, use cases, and usage statistics. \
💡 Use the 'id' parameter from list_notebooks to access specific notebooks.",
                "mimeType": "application/json"
            }]
        })
    }

    // -----------------------------------------------------------------------
    // resources/read
    // -----------------------------------------------------------------------

    pub fn handle_read_resource(&self, params: &Value) -> anyhow::Result<Value> {
        let uri = params["uri"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing 'uri' in resource read request"))?;

        tracing::info!("[MCP] read_resource: {uri}");

        // notebooklm://library — full library snapshot
        if uri == "notebooklm://library" {
            return Ok(self.read_library_resource(uri));
        }

        // notebooklm://library/{id} — single notebook
        if let Some(encoded_id) = uri.strip_prefix("notebooklm://library/") {
            return self.read_notebook_resource(uri, encoded_id);
        }

        // notebooklm://metadata — legacy active notebook
        if uri == "notebooklm://metadata" {
            return self.read_metadata_resource(uri);
        }

        Err(anyhow!("Unknown resource: {uri}"))
    }

    fn read_library_resource(&self, uri: &str) -> Value {
        let notebooks = self.library.list_notebooks();
        let stats = self.library.get_stats();
        let active = self.library.get_active_notebook();

        let library_data = json!({
            "active_notebook": active.as_ref().map(|n| json!({
                "id": n.id,
                "name": n.name,
                "description": n.description,
                "topics": n.topics
            })),
            "notebooks": notebooks.iter().map(|nb| json!({
                "id": nb.id,
                "name": nb.name,
                "description": nb.description,
                "topics": nb.topics,
                "content_types": nb.content_types,
                "use_cases": nb.use_cases,
                "url": nb.url,
                "use_count": nb.use_count,
                "last_used": nb.last_used,
                "tags": nb.tags
            })).collect::<Vec<_>>(),
            "stats": stats
        });

        json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(&library_data).unwrap_or_default()
            }]
        })
    }

    fn read_notebook_resource(&self, uri: &str, encoded_id: &str) -> anyhow::Result<Value> {
        if encoded_id.is_empty() {
            return Err(anyhow!("Notebook resource requires an ID (e.g. notebooklm://library/{{id}})"));
        }

        let id = urlencoding_decode(encoded_id)?;

        // Validate ID format
        if !id.chars().all(|c| c.is_alphanumeric() || c == '-') || id.len() > 63 {
            return Err(anyhow!(
                "Invalid notebook identifier: {encoded_id}. IDs may only contain letters, numbers, and hyphens."
            ));
        }

        let notebook = self
            .library
            .get_notebook(&id)
            .ok_or_else(|| anyhow!("Notebook not found: {id}"))?;

        Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(&notebook).unwrap_or_default()
            }]
        }))
    }

    fn read_metadata_resource(&self, uri: &str) -> anyhow::Result<Value> {
        let active = self
            .library
            .get_active_notebook()
            .ok_or_else(|| anyhow!("No active notebook. Use notebooklm://library to see all notebooks."))?;

        let metadata = json!({
            "description": active.description,
            "topics": active.topics,
            "content_types": active.content_types,
            "use_cases": active.use_cases,
            "notebook_url": active.url,
            "notebook_id": active.id,
            "last_used": active.last_used,
            "use_count": active.use_count,
            "note": "DEPRECATED: Use notebooklm://library or notebooklm://library/{id} instead"
        });

        Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(&metadata).unwrap_or_default()
            }]
        }))
    }

    // -----------------------------------------------------------------------
    // completion/complete
    // -----------------------------------------------------------------------

    pub fn handle_complete(&self, params: &Value) -> Value {
        let ref_type = params["ref"]["type"].as_str().unwrap_or("");
        let ref_uri = params["ref"]["uri"].as_str().unwrap_or("");
        let arg_name = params["argument"]["name"].as_str().unwrap_or("");
        let arg_value = params["argument"]["value"].as_str().unwrap_or("");

        if ref_type == "ref/resource"
            && ref_uri == "notebooklm://library/{id}"
            && arg_name == "id"
        {
            let values = self.complete_notebook_ids(arg_value);
            return json!({
                "completion": {
                    "values": values,
                    "total": values.len()
                }
            });
        }

        json!({ "completion": { "values": [], "total": 0 } })
    }

    fn complete_notebook_ids(&self, prefix: &str) -> Vec<String> {
        let q = prefix.to_lowercase();
        self.library
            .list_notebooks()
            .into_iter()
            .filter(|nb| nb.id.to_lowercase().contains(&q))
            .map(|nb| nb.id)
            .take(50)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Simple percent-decoding (avoids pulling in an HTTP crate)
// ---------------------------------------------------------------------------

fn urlencoding_decode(s: &str) -> anyhow::Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = bytes.next().ok_or_else(|| anyhow!("Truncated percent-encoding"))?;
            let lo = bytes.next().ok_or_else(|| anyhow!("Truncated percent-encoding"))?;
            let hex = [hi, lo];
            let hex_str = std::str::from_utf8(&hex).map_err(|e| anyhow!("{e}"))?;
            let byte = u8::from_str_radix(hex_str, 16).map_err(|e| anyhow!("{e}"))?;
            out.push(byte as char);
        } else {
            out.push(b as char);
        }
    }
    Ok(out)
}
