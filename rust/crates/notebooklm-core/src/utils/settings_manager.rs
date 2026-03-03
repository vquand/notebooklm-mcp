//! Settings manager (Rust rewrite of src/utils/settings-manager.ts)
//!
//! Manages tool profiles and disabled-tool lists in a persistent `settings.json`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use crate::config::config;

// ---------------------------------------------------------------------------
// Profile names
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProfileName {
    Minimal,
    Standard,
    #[default]
    Full,
}

// ---------------------------------------------------------------------------
// Tool allowlists per profile
// ---------------------------------------------------------------------------

static PROFILES: LazyLock<HashMap<ProfileName, Vec<&'static str>>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert(
        ProfileName::Minimal,
        vec![
            "ask_question",
            "get_health",
            "list_notebooks",
            "select_notebook",
            "get_notebook",
        ],
    );
    m.insert(
        ProfileName::Standard,
        vec![
            "ask_question",
            "get_health",
            "list_notebooks",
            "select_notebook",
            "get_notebook",
            "setup_auth",
            "list_sessions",
            "add_notebook",
            "update_notebook",
            "search_notebooks",
        ],
    );
    // "full" is a wildcard — all tools are allowed
    m.insert(ProfileName::Full, vec!["*"]);
    m
});

// ---------------------------------------------------------------------------
// Settings struct
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub profile: ProfileName,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_settings: Option<serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            profile: ProfileName::Full,
            disabled_tools: vec![],
            custom_settings: None,
        }
    }
}

// ---------------------------------------------------------------------------
// SettingsManager
// ---------------------------------------------------------------------------

pub struct SettingsManager {
    settings_path: PathBuf,
    state: RwLock<Settings>,
}

impl SettingsManager {
    pub fn new() -> Self {
        let settings_path = config().config_dir.join("settings.json");
        let state = Self::load_from_disk(&settings_path);

        Self {
            settings_path,
            state: RwLock::new(state),
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Effective settings = file settings merged with env-var overrides.
    pub fn get_effective_settings(&self) -> Settings {
        let base = self.state.read().unwrap().clone();

        // NOTEBOOKLM_PROFILE env var overrides the file profile
        let profile = match std::env::var("NOTEBOOKLM_PROFILE")
            .as_deref()
            .unwrap_or("")
        {
            "minimal" if PROFILES.contains_key(&ProfileName::Minimal) => ProfileName::Minimal,
            "standard" => ProfileName::Standard,
            "full" => ProfileName::Full,
            _ => base.profile,
        };

        // NOTEBOOKLM_DISABLED_TOOLS extends the file list
        let mut disabled = base.disabled_tools.clone();
        if let Ok(env_disabled) = std::env::var("NOTEBOOKLM_DISABLED_TOOLS") {
            for tool in env_disabled.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if !disabled.iter().any(|d| d == tool) {
                    disabled.push(tool.to_string());
                }
            }
        }

        Settings {
            profile,
            disabled_tools: disabled,
            custom_settings: base.custom_settings,
        }
    }

    /// Filter a list of tool names according to the effective profile + disabled list.
    pub fn filter_tool_names<'a>(&self, all_tools: &[&'a str]) -> Vec<&'a str> {
        let settings = self.get_effective_settings();
        let allowed = PROFILES
            .get(&settings.profile)
            .expect("profile exists in PROFILES");
        let is_wildcard = allowed.contains(&"*");

        all_tools
            .iter()
            .copied()
            .filter(|&name| {
                if !is_wildcard && !allowed.contains(&name) {
                    return false;
                }
                !settings.disabled_tools.iter().any(|d| d == name)
            })
            .collect()
    }

    /// Persist a partial settings update to disk.
    pub fn save_settings(&self, patch: SettingsPatch) -> Result<()> {
        let mut guard = self.state.write().unwrap();
        if let Some(p) = patch.profile { guard.profile = p; }
        if let Some(dt) = patch.disabled_tools { guard.disabled_tools = dt; }
        if let Some(cs) = patch.custom_settings { guard.custom_settings = Some(cs); }

        let data = serde_json::to_string_pretty(&*guard)?;
        if let Some(parent) = self.settings_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&self.settings_path, data)
            .with_context(|| format!("Writing settings to {}", self.settings_path.display()))?;
        Ok(())
    }

    pub fn settings_path(&self) -> &PathBuf {
        &self.settings_path
    }

    pub fn profiles() -> &'static HashMap<ProfileName, Vec<&'static str>> {
        &PROFILES
    }

    // -----------------------------------------------------------------------
    // Private
    // -----------------------------------------------------------------------

    fn load_from_disk(path: &PathBuf) -> Settings {
        if path.exists() {
            if let Ok(data) = std::fs::read_to_string(path) {
                if let Ok(s) = serde_json::from_str::<Settings>(&data) {
                    tracing::info!("Settings loaded from {}", path.display());
                    return s;
                }
            }
            tracing::warn!("Failed to parse settings; using defaults");
        }
        Settings::default()
    }
}

impl Default for SettingsManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Patch helper (for partial updates via CLI / tools)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct SettingsPatch {
    pub profile: Option<ProfileName>,
    pub disabled_tools: Option<Vec<String>>,
    pub custom_settings: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn manager_in_tempdir(tmp: &TempDir) -> SettingsManager {
        let settings_path = tmp.path().join("settings.json");
        SettingsManager {
            settings_path,
            state: RwLock::new(Settings::default()),
        }
    }

    #[test]
    fn default_profile_is_full() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_in_tempdir(&tmp);
        assert_eq!(mgr.get_effective_settings().profile, ProfileName::Full);
    }

    #[test]
    fn full_profile_allows_all_tools() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_in_tempdir(&tmp);
        let all = ["ask_question", "get_health", "cleanup_data", "re_auth"];
        let filtered = mgr.filter_tool_names(&all);
        assert_eq!(filtered.len(), 4);
    }

    #[test]
    fn minimal_profile_restricts_tools() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_in_tempdir(&tmp);
        mgr.save_settings(SettingsPatch {
            profile: Some(ProfileName::Minimal),
            ..Default::default()
        })
        .unwrap();

        let all = ["ask_question", "cleanup_data", "re_auth", "get_health"];
        let filtered = mgr.filter_tool_names(&all);
        // cleanup_data and re_auth are not in minimal profile
        assert!(filtered.contains(&"ask_question"));
        assert!(filtered.contains(&"get_health"));
        assert!(!filtered.contains(&"cleanup_data"));
        assert!(!filtered.contains(&"re_auth"));
    }

    #[test]
    fn disabled_tools_are_excluded() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_in_tempdir(&tmp);
        mgr.save_settings(SettingsPatch {
            disabled_tools: Some(vec!["get_health".into()]),
            ..Default::default()
        })
        .unwrap();

        let filtered = mgr.filter_tool_names(&["ask_question", "get_health"]);
        assert!(filtered.contains(&"ask_question"));
        assert!(!filtered.contains(&"get_health"));
    }

    #[test]
    fn settings_round_trip() {
        let tmp = TempDir::new().unwrap();
        let mgr = manager_in_tempdir(&tmp);
        mgr.save_settings(SettingsPatch {
            profile: Some(ProfileName::Standard),
            disabled_tools: Some(vec!["re_auth".into()]),
            ..Default::default()
        })
        .unwrap();

        // Reload from disk
        let loaded = SettingsManager::load_from_disk(&mgr.settings_path);
        assert_eq!(loaded.profile, ProfileName::Standard);
        assert_eq!(loaded.disabled_tools, ["re_auth"]);
    }
}
