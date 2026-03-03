//! NotebookLM Library types (Rust rewrite of src/library/types.ts)

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// NotebookEntry
// ---------------------------------------------------------------------------

/// A single notebook entry in the persistent library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookEntry {
    // Identification
    /// Unique slug ID (e.g. "n8n-docs")
    pub id: String,
    /// NotebookLM URL
    pub url: String,
    /// Human-readable display name
    pub name: String,

    // Metadata for Claude's autonomous decision-making
    pub description: String,
    pub topics: Vec<String>,
    pub content_types: Vec<String>,
    pub use_cases: Vec<String>,

    // Usage tracking
    /// ISO 8601 timestamp when added
    pub added_at: String,
    /// ISO 8601 timestamp of last use
    pub last_used: String,
    pub use_count: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Library (the persisted JSON document)
// ---------------------------------------------------------------------------

/// The complete notebook library as stored in `library.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub notebooks: Vec<NotebookEntry>,
    pub active_notebook_id: Option<String>,
    /// ISO 8601 timestamp of last modification
    pub last_modified: String,
    /// Format version for future migrations
    pub version: String,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            notebooks: vec![],
            active_notebook_id: None,
            last_modified: chrono::Utc::now().to_rfc3339(),
            version: "1.0.0".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Input types
// ---------------------------------------------------------------------------

/// Input for `add_notebook` tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddNotebookInput {
    pub url: String,
    pub name: String,
    pub description: String,
    pub topics: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_cases: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// Input for `update_notebook` tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateNotebookInput {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topics: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_cases: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStats {
    pub total_notebooks: usize,
    pub active_notebook: Option<String>,
    pub most_used_notebook: Option<String>,
    pub total_queries: u64,
    pub last_modified: String,
}
