//! Shared Context Manager
//! (Rust rewrite of src/session/shared-context-manager.ts)
//!
//! Manages ONE global persistent Chrome instance shared by ALL browser sessions.
//! This is critical for fingerprint consistency: Google tracks Canvas, WebGL, Audio
//! and other browser fingerprints — one persistent Chrome profile = one fingerprint.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use tokio::sync::Mutex;

use crate::config::{config, ProfileStrategy};

// ---------------------------------------------------------------------------
// Internal state held while the browser is alive
// ---------------------------------------------------------------------------

struct BrowserState {
    browser: Browser,
    created_at: Instant,
    profile_dir: PathBuf,
    headless: bool,
}

// ---------------------------------------------------------------------------
// SharedContextManager
// ---------------------------------------------------------------------------

pub struct SharedContextManager {
    state: Arc<Mutex<Option<BrowserState>>>,
}

impl SharedContextManager {
    pub fn new(_auth: Arc<crate::auth::AuthManager>) -> Self {
        let cfg = config();
        if cfg.cleanup_instances_on_startup {
            let instances_dir = cfg.chrome_instances_dir.clone();
            tokio::spawn(async move {
                prune_isolated_profiles(&instances_dir, None).await;
            });
        }
        tracing::info!("SharedContextManager initialised (persistent Chrome profile)");
        tracing::info!("  Profile: {}", cfg.chrome_profile_dir.display());
        Self {
            state: Arc::new(Mutex::new(None)),
        }
    }

    // -----------------------------------------------------------------------
    // Public: allocate a new Page from the shared Browser
    // -----------------------------------------------------------------------

    /// Get the shared browser, launch it if needed, and open a new blank tab.
    pub async fn new_page(&self, show_browser: Option<bool>) -> Result<Page> {
        let mut guard = self.state.lock().await;

        let target_headless = match show_browser {
            Some(show) => !show,
            None => config().headless,
        };

        // Close existing browser if headless mode changed
        if let Some(bs) = guard.as_ref() {
            if bs.headless != target_headless {
                tracing::info!("Browser mode change detected — recreating...");
                guard.take(); // drops BrowserState → Chrome exits
            }
        }

        // Check if browser is still alive (pages() succeeds on a live browser)
        if let Some(bs) = guard.as_ref() {
            if bs.browser.pages().await.is_err() {
                tracing::warn!("Stale browser detected — recreating...");
                guard.take();
            }
        }

        // Launch if needed
        if guard.is_none() {
            let (browser, profile_dir) = self.launch_browser(target_headless).await?;
            *guard = Some(BrowserState {
                browser,
                created_at: Instant::now(),
                profile_dir,
                headless: target_headless,
            });
            tracing::info!("Browser launched (headless={target_headless})");
        }

        // Open a new blank tab
        let page = guard
            .as_ref()
            .unwrap()
            .browser
            .new_page("about:blank")
            .await
            .map_err(|e| anyhow!("Failed to open new page: {e}"))?;

        Ok(page)
    }

    // -----------------------------------------------------------------------
    // Public: shut down
    // -----------------------------------------------------------------------

    pub async fn close(&self) {
        let mut guard = self.state.lock().await;
        if let Some(bs) = guard.take() {
            tracing::info!("Closing shared browser...");
            // Drop closes the browser process
            drop(bs);
        }

        let cfg = config();
        if cfg.cleanup_instances_on_shutdown {
            prune_isolated_profiles(&cfg.chrome_instances_dir, None).await;
        }
    }

    // -----------------------------------------------------------------------
    // Public: info for get_health
    // -----------------------------------------------------------------------

    pub async fn get_context_info(&self) -> serde_json::Value {
        let guard = self.state.lock().await;
        match guard.as_ref() {
            None => serde_json::json!({
                "exists": false,
                "user_data_dir": config().chrome_profile_dir.display().to_string(),
                "persistent": true
            }),
            Some(bs) => {
                let age_secs = bs.created_at.elapsed().as_secs_f64();
                serde_json::json!({
                    "exists": true,
                    "age_seconds": age_secs,
                    "age_hours": age_secs / 3600.0,
                    "user_data_dir": bs.profile_dir.display().to_string(),
                    "headless": bs.headless,
                    "persistent": true
                })
            }
        }
    }

    // -----------------------------------------------------------------------
    // Private: launch Chrome
    // -----------------------------------------------------------------------

    async fn launch_browser(&self, headless: bool) -> Result<(Browser, PathBuf)> {
        let cfg = config();
        let base_profile = cfg.chrome_profile_dir.clone();

        match cfg.profile_strategy {
            ProfileStrategy::Isolated => {
                let dir = prepare_isolated_profile(&base_profile).await?;
                let browser = do_launch(&dir, headless).await?;
                Ok((browser, dir))
            }
            ProfileStrategy::Single => {
                // Hard fail if profile is locked
                let browser = do_launch(&base_profile, headless).await?;
                Ok((browser, base_profile))
            }
            ProfileStrategy::Auto => {
                // Try base profile; fall back to isolated if locked
                match do_launch(&base_profile, headless).await {
                    Ok(browser) => Ok((browser, base_profile)),
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("ProcessSingleton")
                            || msg.contains("SingletonLock")
                            || msg.contains("already in use")
                        {
                            tracing::warn!(
                                "Base profile locked — falling back to isolated profile"
                            );
                            let dir = prepare_isolated_profile(&base_profile).await?;
                            let browser = do_launch(&dir, headless).await?;
                            Ok((browser, dir))
                        } else {
                            Err(e)
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Launch helper — builds BrowserConfig and spawns the CDP handler
// ---------------------------------------------------------------------------

async fn do_launch(profile_dir: &PathBuf, headless: bool) -> Result<Browser> {
    std::fs::create_dir_all(profile_dir).ok();

    let mut builder = BrowserConfig::builder()
        .user_data_dir(profile_dir)
        .arg("--disable-blink-features=AutomationControlled")
        .arg("--disable-dev-shm-usage")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-infobars")
        .arg("--lang=en-US");

    // Apply explicit executable if configured
    let exe = config().chrome_executable.clone();
    if let Some(ref path) = exe {
        tracing::info!("Using browser executable: {}", path.display());
        builder = builder.chrome_executable(path);
    }

    if headless {
        builder = builder.new_headless_mode();
    } else {
        builder = builder.with_head().window_size(1280, 900);
    }

    let config_built = builder
        .build()
        .map_err(|e| anyhow!("BrowserConfig build failed: {e}"))?;

    let (browser, mut handler) = Browser::launch(config_built)
        .await
        .map_err(|e| anyhow!("Failed to launch browser: {e}. Ensure Google Chrome or Microsoft Edge is installed."))?;

    // CDP handler MUST be continuously polled — spawn a background task
    tokio::spawn(async move {
        while handler.next().await.is_some() {}
    });

    if !headless {
        let browser_name = exe
            .as_deref()
            .and_then(|p| p.to_str()?.split('/').last().map(|s| s.to_string()))
            .unwrap_or_else(|| "Chrome/Edge".to_string());
        tracing::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        tracing::info!("  Browser launched: {browser_name}");
        tracing::info!("  A new browser window opened with the Google sign-in page.");
        tracing::info!("  ➜ Switch to that window and log in with your Google account.");
        tracing::info!("  NOTE: if {browser_name} was already open, the new window may");
        tracing::info!("  be hidden behind existing windows — check your Dock/taskbar.");
        tracing::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }

    Ok(browser)
}

// ---------------------------------------------------------------------------
// Isolated profile helpers
// ---------------------------------------------------------------------------

async fn prepare_isolated_profile(base_profile: &PathBuf) -> Result<PathBuf> {
    let cfg = config();
    let stamp = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let dir = cfg.chrome_instances_dir.join(format!("instance-{stamp}"));
    std::fs::create_dir_all(&dir)?;

    // Optionally clone the base profile for fingerprint consistency
    if cfg.clone_profile_on_isolated && base_profile.exists() {
        tracing::info!("Cloning base Chrome profile into isolated instance...");
        copy_profile_dir(base_profile, &dir).await;
        tracing::info!("Profile clone complete");
    }

    Ok(dir)
}

async fn copy_profile_dir(src: &PathBuf, dst: &PathBuf) {
    use walkdir::WalkDir;
    for entry in WalkDir::new(src).min_depth(1) {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy();
        // Skip lock files and singletons
        if name.starts_with("Singleton")
            || name.ends_with(".lock")
            || name.ends_with(".tmp")
        {
            continue;
        }
        let rel = match entry.path().strip_prefix(src) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).ok();
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::copy(entry.path(), &target).ok();
        }
    }
}

async fn prune_isolated_profiles(instances_dir: &PathBuf, current: Option<&PathBuf>) {
    let cfg = config();
    let Ok(entries) = std::fs::read_dir(instances_dir) else {
        return;
    };

    let now = std::time::SystemTime::now();
    let ttl_secs = cfg.instance_profile_ttl_hours * 3600;
    let max_count = cfg.instance_profile_max_count as usize;

    let mut dirs: Vec<(PathBuf, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let p = e.path();
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
            Some((p, mtime))
        })
        .collect();

    // Sort newest first
    dirs.sort_by(|a, b| b.1.cmp(&a.1));

    let mut kept = 0usize;
    for (dir, mtime) in &dirs {
        if let Some(cur) = current {
            if dir == cur {
                kept += 1;
                continue;
            }
        }
        let age_secs = now.duration_since(*mtime).unwrap_or_default().as_secs();
        let over_ttl = ttl_secs > 0 && age_secs > ttl_secs;
        let over_count = kept >= max_count;
        if over_ttl || over_count {
            tracing::debug!("Pruning isolated profile: {}", dir.display());
            std::fs::remove_dir_all(dir).ok();
        } else {
            kept += 1;
        }
    }
}
