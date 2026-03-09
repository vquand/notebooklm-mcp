//! Configuration for NotebookLM MCP Server (Rust rewrite of src/config.ts)
//!
//! Config Priority (highest to lowest):
//!   1. Hardcoded defaults  (works out of the box)
//!   2. Environment variables  (optional, for advanced users)
//!   3. Tool parameters  (passed by Claude at runtime via `apply_browser_options`)

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Google NotebookLM auth URL (used by setup_auth tool)
pub const NOTEBOOKLM_AUTH_URL: &str = concat!(
    "https://accounts.google.com/v3/signin/identifier",
    "?continue=https%3A%2F%2Fnotebooklm.google.com%2F",
    "&flowName=GlifWebSignIn&flowEntry=ServiceLogin"
);

// ---------------------------------------------------------------------------
// Config struct
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Config {
    // NotebookLM — optional legacy default notebook
    pub notebook_url: String,

    // Browser
    pub headless: bool,
    pub chrome_executable: Option<PathBuf>,
    pub browser_timeout: u64, // milliseconds
    pub viewport_width: u32,
    pub viewport_height: u32,

    // Session management
    pub max_sessions: u32,
    pub session_timeout: u64, // seconds

    // Authentication
    pub auto_login_enabled: bool,
    pub login_email: String,
    pub login_password: String,
    pub auto_login_timeout_ms: u64,

    // Stealth settings
    pub stealth_enabled: bool,
    pub stealth_random_delays: bool,
    pub stealth_human_typing: bool,
    pub stealth_mouse_movements: bool,
    pub typing_wpm_min: u32,
    pub typing_wpm_max: u32,
    pub min_delay_ms: u64,
    pub max_delay_ms: u64,

    // Paths (cross-platform via `directories` crate)
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub browser_state_dir: PathBuf,
    pub chrome_profile_dir: PathBuf,
    pub chrome_instances_dir: PathBuf,

    // Library metadata defaults
    pub notebook_description: String,
    pub notebook_topics: Vec<String>,
    pub notebook_content_types: Vec<String>,
    pub notebook_use_cases: Vec<String>,

    // Multi-instance profile strategy
    pub profile_strategy: ProfileStrategy,
    pub clone_profile_on_isolated: bool,
    pub cleanup_instances_on_startup: bool,
    pub cleanup_instances_on_shutdown: bool,
    pub instance_profile_ttl_hours: u64,
    pub instance_profile_max_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileStrategy {
    /// Try shared profile; fall back to isolated if locked (default)
    Auto,
    /// Always use a single shared profile
    Single,
    /// Always create an isolated per-process profile
    Isolated,
}

// ---------------------------------------------------------------------------
// Singleton accessor
// ---------------------------------------------------------------------------

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Return a reference to the global `Config`.
///
/// Reads from env vars on first call (lazy, thread-safe).
/// Call `dotenvy::dotenv().ok()` in `main` *before* the first call here.
pub fn config() -> &'static Config {
    CONFIG.get_or_init(Config::build_from_env)
}

// ---------------------------------------------------------------------------
// Directory initialisation
// ---------------------------------------------------------------------------

/// Create all required data directories (idempotent).
/// Called once in `main` after tracing is initialised.
pub fn ensure_directories() {
    let cfg = config();
    for dir in &[
        &cfg.data_dir,
        &cfg.browser_state_dir,
        &cfg.chrome_profile_dir,
        &cfg.chrome_instances_dir,
    ] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("Could not create directory {}: {e}", dir.display());
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

impl Config {
    fn build_from_env() -> Self {
        // Resolve cross-platform app directories
        // qualifier="" + organisation="" disables default "com.company." prefix
        let proj = ProjectDirs::from("", "", "notebooklm-mcp")
            .expect("could not determine home directory");

        let data_dir = proj.data_dir().to_path_buf();
        let config_dir = proj.config_dir().to_path_buf();

        Config {
            notebook_url: env_string("NOTEBOOK_URL", ""),

            headless: env_bool("HEADLESS", true),
            chrome_executable: env_chrome_executable(),
            browser_timeout: env_u64("BROWSER_TIMEOUT", 30_000),
            viewport_width: env_u32("VIEWPORT_WIDTH", 1024),
            viewport_height: env_u32("VIEWPORT_HEIGHT", 768),

            max_sessions: env_u32("MAX_SESSIONS", 10),
            session_timeout: env_u64("SESSION_TIMEOUT", 900),

            auto_login_enabled: env_bool("AUTO_LOGIN_ENABLED", false),
            login_email: env_string("LOGIN_EMAIL", ""),
            login_password: env_string("LOGIN_PASSWORD", ""),
            auto_login_timeout_ms: env_u64("AUTO_LOGIN_TIMEOUT_MS", 120_000),

            stealth_enabled: env_bool("STEALTH_ENABLED", true),
            stealth_random_delays: env_bool("STEALTH_RANDOM_DELAYS", true),
            stealth_human_typing: env_bool("STEALTH_HUMAN_TYPING", true),
            stealth_mouse_movements: env_bool("STEALTH_MOUSE_MOVEMENTS", true),
            typing_wpm_min: env_u32("TYPING_WPM_MIN", 160),
            typing_wpm_max: env_u32("TYPING_WPM_MAX", 240),
            min_delay_ms: env_u64("MIN_DELAY_MS", 100),
            max_delay_ms: env_u64("MAX_DELAY_MS", 400),

            browser_state_dir: data_dir.join("browser_state"),
            chrome_profile_dir: data_dir.join("chrome_profile"),
            chrome_instances_dir: data_dir.join("chrome_profile_instances"),
            data_dir,
            config_dir,

            notebook_description: env_string("NOTEBOOK_DESCRIPTION", "General knowledge base"),
            notebook_topics: env_vec("NOTEBOOK_TOPICS", vec!["General topics".into()]),
            notebook_content_types: env_vec(
                "NOTEBOOK_CONTENT_TYPES",
                vec!["documentation".into(), "examples".into()],
            ),
            notebook_use_cases: env_vec(
                "NOTEBOOK_USE_CASES",
                vec!["General research".into()],
            ),

            profile_strategy: match std::env::var("NOTEBOOK_PROFILE_STRATEGY")
                .as_deref()
                .unwrap_or("")
            {
                "single" => ProfileStrategy::Single,
                "isolated" => ProfileStrategy::Isolated,
                _ => ProfileStrategy::Auto,
            },
            clone_profile_on_isolated: env_bool("NOTEBOOK_CLONE_PROFILE", false),
            cleanup_instances_on_startup: env_bool("NOTEBOOK_CLEANUP_ON_STARTUP", true),
            cleanup_instances_on_shutdown: env_bool("NOTEBOOK_CLEANUP_ON_SHUTDOWN", true),
            instance_profile_ttl_hours: env_u64("NOTEBOOK_INSTANCE_TTL_HOURS", 72),
            instance_profile_max_count: env_u32("NOTEBOOK_INSTANCE_MAX_COUNT", 20),
        }
    }
}

// ---------------------------------------------------------------------------
// Browser options (per-call overrides — mirrors BrowserOptions in config.ts)
// ---------------------------------------------------------------------------

/// Runtime overrides passed by Claude via tool parameters.
/// Does NOT mutate the global `Config`; returns an owned clone.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrowserOptions {
    /// Show browser window (inverse of headless)
    pub show: Option<bool>,
    pub headless: Option<bool>,
    pub timeout_ms: Option<u64>,
    pub stealth: Option<StealthOptions>,
    pub viewport: Option<ViewportOptions>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StealthOptions {
    pub enabled: Option<bool>,
    pub random_delays: Option<bool>,
    pub human_typing: Option<bool>,
    pub mouse_movements: Option<bool>,
    pub typing_wpm_min: Option<u32>,
    pub typing_wpm_max: Option<u32>,
    pub delay_min_ms: Option<u64>,
    pub delay_max_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViewportOptions {
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Apply per-call `BrowserOptions` on top of the global `Config`.
/// `legacy_show_browser` maps the old `show_browser` parameter.
pub fn apply_browser_options(
    options: Option<&BrowserOptions>,
    legacy_show_browser: Option<bool>,
) -> Config {
    let mut cfg = config().clone();

    if let Some(show) = legacy_show_browser {
        cfg.headless = !show;
    }

    if let Some(opts) = options {
        if let Some(show) = opts.show {
            cfg.headless = !show;
        }
        if let Some(headless) = opts.headless {
            cfg.headless = headless;
        }
        if let Some(ms) = opts.timeout_ms {
            cfg.browser_timeout = ms;
        }
        if let Some(s) = &opts.stealth {
            if let Some(v) = s.enabled { cfg.stealth_enabled = v; }
            if let Some(v) = s.random_delays { cfg.stealth_random_delays = v; }
            if let Some(v) = s.human_typing { cfg.stealth_human_typing = v; }
            if let Some(v) = s.mouse_movements { cfg.stealth_mouse_movements = v; }
            if let Some(v) = s.typing_wpm_min { cfg.typing_wpm_min = v; }
            if let Some(v) = s.typing_wpm_max { cfg.typing_wpm_max = v; }
            if let Some(v) = s.delay_min_ms { cfg.min_delay_ms = v; }
            if let Some(v) = s.delay_max_ms { cfg.max_delay_ms = v; }
        }
        if let Some(vp) = &opts.viewport {
            if let Some(w) = vp.width { cfg.viewport_width = w; }
            if let Some(h) = vp.height { cfg.viewport_height = h; }
        }
    }

    cfg
}

// ---------------------------------------------------------------------------
// Env-var helpers
// ---------------------------------------------------------------------------

fn env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key).as_deref() {
        Ok("true") | Ok("1") => true,
        Ok("false") | Ok("0") => false,
        _ => default,
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Resolve the Chrome/Chromium-compatible executable to use.
///
/// Priority:
///   1. `CHROME_PATH` env var (explicit override)
///   2. Well-known macOS paths (Chrome → Edge → Chromium)
///   3. `None` → chromiumoxide uses its own auto-discovery
fn env_chrome_executable() -> Option<PathBuf> {
    // 1. Explicit override
    if let Ok(p) = std::env::var("CHROME_PATH") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return Some(path);
        }
        tracing::warn!("CHROME_PATH={p} does not exist — falling back to auto-detect");
    }

    // 2. Auto-detect common macOS paths
    #[cfg(target_os = "macos")]
    {
        let candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ];
        for p in &candidates {
            let path = PathBuf::from(p);
            if path.exists() {
                tracing::info!("Auto-detected browser: {p}");
                return Some(path);
            }
        }
    }

    None
}

fn env_vec(key: &str, default: Vec<String>) -> Vec<String> {
    match std::env::var(key) {
        Ok(val) if !val.is_empty() => val
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => default,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_bool_parses_correctly() {
        assert!(env_bool("__NONEXISTENT__", true));
        assert!(!env_bool("__NONEXISTENT__", false));
    }

    #[test]
    fn env_vec_splits_comma_separated() {
        std::env::set_var("__TEST_VEC__", "a, b, c");
        let v = env_vec("__TEST_VEC__", vec![]);
        assert_eq!(v, ["a", "b", "c"]);
        std::env::remove_var("__TEST_VEC__");
    }

    #[test]
    fn config_paths_are_non_empty() {
        let cfg = config();
        assert!(!cfg.data_dir.as_os_str().is_empty());
        assert!(!cfg.config_dir.as_os_str().is_empty());
        assert_eq!(cfg.browser_state_dir, cfg.data_dir.join("browser_state"));
        assert_eq!(cfg.chrome_profile_dir, cfg.data_dir.join("chrome_profile"));
    }
}
