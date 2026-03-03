//! NotebookLM Library manager (Rust rewrite of src/library/notebook-library.ts)
//!
//! Manages a persistent JSON library of NotebookLM notebooks.
//! Thread-safe via `RwLock<Library>` — reads are concurrent, writes exclusive.

use std::path::PathBuf;
use std::sync::RwLock;

use anyhow::{Context, Result};

use crate::config::config;
use super::types::*;

// ---------------------------------------------------------------------------
// NotebookLibrary
// ---------------------------------------------------------------------------

pub struct NotebookLibrary {
    library_path: PathBuf,
    state: RwLock<Library>,
}

impl NotebookLibrary {
    /// Load (or create) the library from disk.
    pub fn new() -> Result<Self> {
        let library_path = config().data_dir.join("library.json");
        let state = Self::load_from_disk(&library_path)?;

        tracing::info!("NotebookLibrary initialised");
        tracing::info!("  path:      {}", library_path.display());
        tracing::info!("  notebooks: {}", state.notebooks.len());
        if let Some(ref id) = state.active_notebook_id {
            tracing::info!("  active:    {id}");
        }

        Ok(Self { library_path, state: RwLock::new(state) })
    }

    // -----------------------------------------------------------------------
    // Public CRUD
    // -----------------------------------------------------------------------

    /// Add a new notebook to the library.
    pub fn add_notebook(&self, input: AddNotebookInput) -> Result<NotebookEntry> {
        tracing::info!("Adding notebook: {}", input.name);

        let mut guard = self.state.write().unwrap();

        let id = Self::generate_id(&guard, &input.name);
        let now = chrono::Utc::now().to_rfc3339();

        let notebook = NotebookEntry {
            id: id.clone(),
            url: input.url,
            name: input.name.clone(),
            description: input.description,
            topics: input.topics,
            content_types: input
                .content_types
                .unwrap_or_else(|| vec!["documentation".into(), "examples".into()]),
            use_cases: input.use_cases.unwrap_or_else(|| {
                vec![
                    format!("Learning about {}", input.name),
                    format!("Implementing features with {}", input.name),
                ]
            }),
            added_at: now.clone(),
            last_used: now,
            use_count: 0,
            tags: input.tags,
        };

        guard.notebooks.push(notebook.clone());

        // First notebook becomes the active one
        if guard.notebooks.len() == 1 {
            guard.active_notebook_id = Some(id.clone());
        }

        Self::persist(&self.library_path, &mut guard)?;
        tracing::info!("Notebook added: {id}");

        Ok(notebook)
    }

    /// List all notebooks.
    pub fn list_notebooks(&self) -> Vec<NotebookEntry> {
        self.state.read().unwrap().notebooks.clone()
    }

    /// Get a notebook by ID.
    pub fn get_notebook(&self, id: &str) -> Option<NotebookEntry> {
        self.state
            .read()
            .unwrap()
            .notebooks
            .iter()
            .find(|n| n.id == id)
            .cloned()
    }

    /// Get the currently active notebook.
    pub fn get_active_notebook(&self) -> Option<NotebookEntry> {
        let guard = self.state.read().unwrap();
        guard
            .active_notebook_id
            .as_deref()
            .and_then(|id| guard.notebooks.iter().find(|n| n.id == id).cloned())
    }

    /// Select a notebook as the active one.
    pub fn select_notebook(&self, id: &str) -> Result<NotebookEntry> {
        tracing::info!("Selecting notebook: {id}");

        let mut guard = self.state.write().unwrap();
        let idx = guard
            .notebooks
            .iter()
            .position(|n| n.id == id)
            .with_context(|| format!("Notebook not found: {id}"))?;

        guard.active_notebook_id = Some(id.to_string());
        guard.notebooks[idx].last_used = chrono::Utc::now().to_rfc3339();

        Self::persist(&self.library_path, &mut guard)?;
        tracing::info!("Active notebook: {id}");

        Ok(guard.notebooks[idx].clone())
    }

    /// Update notebook metadata.
    pub fn update_notebook(&self, input: UpdateNotebookInput) -> Result<NotebookEntry> {
        tracing::info!("Updating notebook: {}", input.id);

        let mut guard = self.state.write().unwrap();
        let idx = guard
            .notebooks
            .iter()
            .position(|n| n.id == input.id)
            .with_context(|| format!("Notebook not found: {}", input.id))?;

        let nb = &mut guard.notebooks[idx];
        if let Some(v) = input.name { nb.name = v; }
        if let Some(v) = input.description { nb.description = v; }
        if let Some(v) = input.topics { nb.topics = v; }
        if let Some(v) = input.content_types { nb.content_types = v; }
        if let Some(v) = input.use_cases { nb.use_cases = v; }
        if let Some(v) = input.tags { nb.tags = Some(v); }
        if let Some(v) = input.url { nb.url = v; }

        let updated = nb.clone();
        Self::persist(&self.library_path, &mut guard)?;
        tracing::info!("Notebook updated: {}", input.id);

        Ok(updated)
    }

    /// Remove a notebook from the library.
    ///
    /// Returns `true` if the notebook was found and removed.
    pub fn remove_notebook(&self, id: &str) -> Result<bool> {
        tracing::info!("Removing notebook: {id}");

        let mut guard = self.state.write().unwrap();
        let before = guard.notebooks.len();
        guard.notebooks.retain(|n| n.id != id);

        if guard.notebooks.len() == before {
            return Ok(false);
        }

        // If we removed the active notebook, promote another
        if guard.active_notebook_id.as_deref() == Some(id) {
            guard.active_notebook_id =
                guard.notebooks.first().map(|n| n.id.clone());
        }

        Self::persist(&self.library_path, &mut guard)?;
        tracing::info!("Notebook removed: {id}");

        Ok(true)
    }

    /// Increment the use count for a notebook and update `last_used`.
    pub fn increment_use_count(&self, id: &str) -> Option<NotebookEntry> {
        let mut guard = self.state.write().unwrap();
        let nb = guard.notebooks.iter_mut().find(|n| n.id == id)?;
        nb.use_count += 1;
        nb.last_used = chrono::Utc::now().to_rfc3339();
        let updated = nb.clone();
        let _ = Self::persist(&self.library_path, &mut guard);
        Some(updated)
    }

    /// Library usage statistics.
    pub fn get_stats(&self) -> LibraryStats {
        let guard = self.state.read().unwrap();
        let total_queries: u64 = guard.notebooks.iter().map(|n| n.use_count).sum();
        let most_used = guard
            .notebooks
            .iter()
            .max_by_key(|n| n.use_count)
            .map(|n| n.id.clone());

        LibraryStats {
            total_notebooks: guard.notebooks.len(),
            active_notebook: guard.active_notebook_id.clone(),
            most_used_notebook: most_used,
            total_queries,
            last_modified: guard.last_modified.clone(),
        }
    }

    /// Full-text search over name, description, topics, and tags.
    pub fn search_notebooks(&self, query: &str) -> Vec<NotebookEntry> {
        let q = query.to_lowercase();
        self.state
            .read()
            .unwrap()
            .notebooks
            .iter()
            .filter(|n| {
                n.name.to_lowercase().contains(&q)
                    || n.description.to_lowercase().contains(&q)
                    || n.topics.iter().any(|t| t.to_lowercase().contains(&q))
                    || n.tags
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .any(|t| t.to_lowercase().contains(&q))
            })
            .cloned()
            .collect()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn load_from_disk(path: &PathBuf) -> Result<Library> {
        if path.exists() {
            let data = std::fs::read_to_string(path)
                .with_context(|| format!("Reading {}", path.display()))?;
            let lib: Library = serde_json::from_str(&data)
                .with_context(|| format!("Parsing {}", path.display()))?;
            tracing::info!("Loaded library ({} notebooks)", lib.notebooks.len());
            return Ok(lib);
        }

        // Create a default empty library
        tracing::info!("Creating new library");
        let lib = Self::create_default_library();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let data = serde_json::to_string_pretty(&lib)?;
        std::fs::write(path, data)
            .with_context(|| format!("Writing {}", path.display()))?;

        Ok(lib)
    }

    fn create_default_library() -> Library {
        let cfg = config();
        let mut lib = Library::default();

        // Auto-populate from CONFIG if a URL + description is set
        let has_config = !cfg.notebook_url.is_empty()
            && !cfg.notebook_description.is_empty()
            && cfg.notebook_description != "General knowledge base";

        if has_config {
            let id = Self::generate_id(&lib, &cfg.notebook_description);
            let now = chrono::Utc::now().to_rfc3339();
            let notebook = NotebookEntry {
                id: id.clone(),
                url: cfg.notebook_url.clone(),
                name: cfg.notebook_description.chars().take(50).collect(),
                description: cfg.notebook_description.clone(),
                topics: cfg.notebook_topics.clone(),
                content_types: cfg.notebook_content_types.clone(),
                use_cases: cfg.notebook_use_cases.clone(),
                added_at: now.clone(),
                last_used: now,
                use_count: 0,
                tags: Some(vec![]),
            };
            lib.active_notebook_id = Some(id);
            lib.notebooks.push(notebook);
        }

        lib
    }

    /// Generate a unique slug ID from a name string.
    ///
    /// Equivalent to the TypeScript `generateId` private method.
    fn generate_id(library: &Library, name: &str) -> String {
        // slug::slugify lowercases + replaces non-alphanum with hyphens + trims hyphens
        let base: String = slug::slugify(name).chars().take(30).collect();
        let base = if base.is_empty() { "notebook".to_string() } else { base };

        let mut id = base.clone();
        let mut counter = 1u32;
        while library.notebooks.iter().any(|n| n.id == id) {
            id = format!("{base}-{counter}");
            counter += 1;
        }

        id
    }

    /// Serialize and write the library to disk (called while holding the write lock).
    fn persist(path: &PathBuf, library: &mut Library) -> Result<()> {
        library.last_modified = chrono::Utc::now().to_rfc3339();
        let data = serde_json::to_string_pretty(&library)?;
        std::fs::write(path, data)
            .with_context(|| format!("Saving library to {}", path.display()))?;
        tracing::debug!("Library saved ({} notebooks)", library.notebooks.len());
        Ok(())
    }
}

impl Default for NotebookLibrary {
    fn default() -> Self {
        Self::new().expect("failed to initialise NotebookLibrary")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn library_in_tempdir(tmp: &TempDir) -> NotebookLibrary {
        // Override the library path by constructing directly
        let library_path = tmp.path().join("library.json");
        let state = Library::default();
        NotebookLibrary {
            library_path,
            state: RwLock::new(state),
        }
    }

    fn sample_input(name: &str) -> AddNotebookInput {
        AddNotebookInput {
            url: format!("https://notebooklm.google.com/notebook/{name}"),
            name: name.to_string(),
            description: format!("Test notebook: {name}"),
            topics: vec!["testing".into()],
            content_types: None,
            use_cases: None,
            tags: None,
        }
    }

    #[test]
    fn add_and_list() {
        let tmp = TempDir::new().unwrap();
        let lib = library_in_tempdir(&tmp);

        let nb = lib.add_notebook(sample_input("my-docs")).unwrap();
        assert_eq!(nb.id, "my-docs");
        assert_eq!(lib.list_notebooks().len(), 1);
    }

    #[test]
    fn first_notebook_becomes_active() {
        let tmp = TempDir::new().unwrap();
        let lib = library_in_tempdir(&tmp);

        lib.add_notebook(sample_input("alpha")).unwrap();
        assert_eq!(lib.get_active_notebook().unwrap().id, "alpha");
    }

    #[test]
    fn search_by_topic() {
        let tmp = TempDir::new().unwrap();
        let lib = library_in_tempdir(&tmp);

        lib.add_notebook(AddNotebookInput {
            topics: vec!["rust".into(), "async".into()],
            ..sample_input("tokio-guide")
        })
        .unwrap();
        lib.add_notebook(sample_input("python-guide")).unwrap();

        let results = lib.search_notebooks("rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "tokio-guide");
    }

    #[test]
    fn remove_active_promotes_another() {
        let tmp = TempDir::new().unwrap();
        let lib = library_in_tempdir(&tmp);

        lib.add_notebook(sample_input("first")).unwrap();
        lib.add_notebook(sample_input("second")).unwrap();
        lib.select_notebook("first").unwrap();

        assert!(lib.remove_notebook("first").unwrap());
        // "second" should now be active
        assert_eq!(lib.get_active_notebook().unwrap().id, "second");
    }

    #[test]
    fn duplicate_names_get_unique_ids() {
        let tmp = TempDir::new().unwrap();
        let lib = library_in_tempdir(&tmp);

        lib.add_notebook(sample_input("my docs")).unwrap();
        let nb2 = lib.add_notebook(sample_input("my docs")).unwrap();
        // Second one should be "my-docs-1"
        assert_eq!(nb2.id, "my-docs-1");
    }

    #[test]
    fn increment_use_count() {
        let tmp = TempDir::new().unwrap();
        let lib = library_in_tempdir(&tmp);

        lib.add_notebook(sample_input("counter")).unwrap();
        lib.increment_use_count("counter");
        lib.increment_use_count("counter");

        let stats = lib.get_stats();
        assert_eq!(stats.total_queries, 2);
    }
}
