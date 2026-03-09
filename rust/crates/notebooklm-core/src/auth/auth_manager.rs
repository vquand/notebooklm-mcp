//! Authentication Manager (Phase 6 — interactive OAuth login)
//!
//! Handles:
//! - Reading saved browser state (Playwright storageState JSON format)
//! - Validating cookie expiry via CDP
//! - Injecting cookies into a chromiumoxide Page
//! - Session storage restore
//! - Interactive Google login (setup_auth / re_auth) via browser automation

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::network::CookieParam;
use serde::Deserialize;

use crate::config::{config, NOTEBOOKLM_AUTH_URL};

// ---------------------------------------------------------------------------
// Critical Google auth cookie names
// ---------------------------------------------------------------------------

const CRITICAL_COOKIE_NAMES: &[&str] = &[
    "SID",
    "HSID",
    "SSID",
    "__Secure-OSID",
    "__Secure-1PSID",
    "__Secure-3PSID",
    "OSID",
    "APISID",
    "SAPISID",
];

// ---------------------------------------------------------------------------
// Playwright storageState format (what the TypeScript server saves)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct PlaywrightCookie {
    name: String,
    value: String,
    #[serde(default)]
    domain: String,
    #[serde(default = "default_path")]
    path: String,
    #[serde(default)]
    expires: f64,
    #[serde(rename = "httpOnly", default)]
    http_only: bool,
    #[serde(default)]
    secure: bool,
}

fn default_path() -> String {
    "/".to_string()
}

#[derive(Debug, Deserialize)]
struct PlaywrightState {
    #[serde(default)]
    cookies: Vec<PlaywrightCookie>,
}

// ---------------------------------------------------------------------------
// AuthManager
// ---------------------------------------------------------------------------

pub struct AuthManager {
    state_path: PathBuf,
    session_path: PathBuf,
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthManager {
    pub fn new() -> Self {
        let cfg = config();
        Self {
            state_path: cfg.browser_state_dir.join("state.json"),
            session_path: cfg.browser_state_dir.join("session.json"),
        }
    }

    // -----------------------------------------------------------------------
    // State file checks
    // -----------------------------------------------------------------------

    pub fn has_saved_state(&self) -> bool {
        self.state_path.exists()
    }

    /// Returns `true` if the state file is older than 24 hours (or absent).
    pub fn is_state_expired(&self) -> bool {
        if !self.state_path.exists() {
            return true;
        }
        std::fs::metadata(&self.state_path)
            .and_then(|m| m.modified())
            .map(|modified| {
                SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or_default()
                    .as_secs()
                    > 24 * 3600
            })
            .unwrap_or(true)
    }

    /// Returns `Some(&path)` if state exists and is not expired, `None` otherwise.
    pub fn get_valid_state_path(&self) -> Option<&PathBuf> {
        if self.has_saved_state() && !self.is_state_expired() {
            Some(&self.state_path)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // Cookie validation via CDP
    // -----------------------------------------------------------------------

    /// Check that at least one critical Google auth cookie is present and not expired.
    pub async fn validate_cookies_expiry(&self, page: &Page) -> bool {
        let cookies = match page.get_cookies().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("validate_cookies_expiry: get_cookies failed: {e}");
                return false;
            }
        };

        if cookies.is_empty() {
            tracing::warn!("validate_cookies_expiry: no cookies in browser");
            return false;
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let critical: Vec<_> = cookies
            .iter()
            .filter(|c| CRITICAL_COOKIE_NAMES.contains(&c.name.as_str()))
            .collect();

        if critical.is_empty() {
            tracing::warn!("validate_cookies_expiry: no critical auth cookies found");
            return false;
        }

        for cookie in &critical {
            // expires == -1 means session cookie (valid while browser is open)
            // Cookie::expires is f64 in chromiumoxide (not Option)
            let exp = cookie.expires;
            if exp > 0.0 && exp < now {
                tracing::warn!("Cookie '{}' has expired", cookie.name);
                return false;
            }
        }

        tracing::debug!(
            "validate_cookies_expiry: {} critical cookies OK",
            critical.len()
        );
        true
    }

    // -----------------------------------------------------------------------
    // Cookie injection (loads state.json → CDP Network.setCookies)
    // -----------------------------------------------------------------------

    /// Read `state.json` (Playwright storageState format) and inject the
    /// cookies into `page` via CDP.
    pub async fn load_auth_state(&self, page: &Page) -> Result<()> {
        let raw = std::fs::read_to_string(&self.state_path)
            .map_err(|e| anyhow!("Cannot read state.json: {e}"))?;

        let state: PlaywrightState = serde_json::from_str(&raw)
            .map_err(|e| anyhow!("Cannot parse state.json: {e}"))?;

        let params: Vec<CookieParam> = state
            .cookies
            .iter()
            .map(|c| {
                let mut p = CookieParam::new(c.name.clone(), c.value.clone());
                if !c.domain.is_empty() {
                    p.domain = Some(c.domain.clone());
                }
                p.path = Some(c.path.clone());
                p.http_only = Some(c.http_only);
                p.secure = Some(c.secure);
                if c.expires > 0.0 {
                    p.expires = Some(chromiumoxide::cdp::browser_protocol::network::TimeSinceEpoch::new(c.expires));
                }
                p
            })
            .collect();

        let n = params.len();
        page.set_cookies(params)
            .await
            .map_err(|e| anyhow!("Failed to inject cookies: {e}"))?;

        tracing::info!("load_auth_state: injected {n} cookies");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Session storage restore
    // -----------------------------------------------------------------------

    /// Read `session.json` and restore `sessionStorage` entries via `page.evaluate`.
    /// Best-effort — errors are logged but not propagated.
    pub async fn load_session_storage(&self, page: &Page) -> Result<()> {
        if !self.session_path.exists() {
            tracing::debug!("load_session_storage: no session.json found");
            return Ok(());
        }

        let raw = std::fs::read_to_string(&self.session_path)
            .map_err(|e| anyhow!("Cannot read session.json: {e}"))?;

        // Verify it's valid JSON before injecting
        let map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&raw).map_err(|e| anyhow!("Cannot parse session.json: {e}"))?;

        let n = map.len();
        if n == 0 {
            return Ok(());
        }

        // Inject via JavaScript
        let js = format!(
            r#"() => {{
                const data = {};
                for (const [k, v] of Object.entries(data)) {{
                    try {{ sessionStorage.setItem(k, typeof v === 'string' ? v : JSON.stringify(v)); }} catch(e) {{}}
                }}
            }}"#,
            raw
        );

        page.evaluate_function(js)
            .await
            .map_err(|e| anyhow!("sessionStorage restore failed: {e}"))?;

        tracing::info!("load_session_storage: restored {n} entries");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Auth info (for get_health tool)
    // -----------------------------------------------------------------------

    pub fn get_auth_info(&self) -> serde_json::Value {
        serde_json::json!({
            "has_saved_state": self.has_saved_state(),
            "state_expired": self.is_state_expired(),
            "authenticated": self.has_saved_state() && !self.is_state_expired(),
            "state_path": self.state_path.display().to_string(),
        })
    }

    // -----------------------------------------------------------------------
    // Phase 6: Interactive login
    // -----------------------------------------------------------------------

    /// Navigate to Google sign-in in `page`, wait for the user to complete login,
    /// then save the resulting cookies to `state.json`.
    ///
    /// Login is detected when:
    ///   1. The page URL changes to `notebooklm.google.com` (redirect after login), OR
    ///   2. Critical Google auth cookies become present and valid.
    ///
    /// `timeout_ms` — how long to wait before giving up (default: 10 minutes).
    pub async fn interactive_login(&self, page: &Page, timeout_ms: u64) -> Result<()> {
        tracing::info!("interactive_login: navigating to Google sign-in...");

        page.goto(NOTEBOOKLM_AUTH_URL)
            .await
            .map_err(|e| anyhow!("Failed to navigate to auth URL: {e}"))?;

        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        let poll_interval = Duration::from_secs(2);

        tracing::info!(
            "interactive_login: waiting for login (timeout {}s)...",
            timeout_ms / 1000
        );

        loop {
            if std::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "Login timed out after {}s — please call setup_auth again to retry",
                    timeout_ms / 1000
                ));
            }

            // Check if the browser has been redirected to NotebookLM.
            // Wrap in a timeout — page.evaluate can hang indefinitely while the
            // page is loading (e.g. after navigating to NotebookLM).
            let current_url: String = match tokio::time::timeout(
                Duration::from_secs(3),
                page.evaluate("window.location.href"),
            )
            .await
            {
                Ok(Ok(obj)) => obj
                    .value()
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_default(),
                Ok(Err(e)) => {
                    tracing::debug!("interactive_login: evaluate error: {e}");
                    String::new()
                }
                Err(_) => {
                    tracing::debug!("interactive_login: evaluate timed out — page busy, checking cookies");
                    String::new()
                }
            };

            tracing::debug!("interactive_login: url = {current_url}");

            if current_url.contains("notebooklm.google.com")
                && !current_url.contains("accounts.google.com")
            {
                tracing::info!("interactive_login: redirect to NotebookLM detected — saving cookies");
                self.save_cookies_from_page(page).await?;
                return Ok(());
            }

            // Fallback: check cookies even if URL hasn't changed yet
            if !current_url.is_empty()
                && !current_url.contains("accounts.google.com/v3/signin")
                && self.validate_cookies_expiry(page).await
            {
                tracing::info!("interactive_login: auth cookies detected — saving cookies");
                self.save_cookies_from_page(page).await?;
                return Ok(());
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Extract all cookies from `page` and write them to `state.json` in
    /// Playwright storageState format so they can be restored on next startup.
    pub async fn save_cookies_from_page(&self, page: &Page) -> Result<()> {
        let cookies = page
            .get_cookies()
            .await
            .map_err(|e| anyhow!("Failed to read cookies from page: {e}"))?;

        let serialized: Vec<serde_json::Value> = cookies
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name":     c.name,
                    "value":    c.value,
                    "domain":   c.domain,
                    "path":     c.path,
                    "expires":  c.expires,
                    "httpOnly": c.http_only,
                    "secure":   c.secure,
                })
            })
            .collect();

        let state = serde_json::json!({ "cookies": serialized });

        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow!("Cannot create browser_state dir: {e}"))?;
        }

        std::fs::write(
            &self.state_path,
            serde_json::to_string_pretty(&state)
                .map_err(|e| anyhow!("JSON serialization failed: {e}"))?,
        )
        .map_err(|e| anyhow!("Cannot write state.json: {e}"))?;

        tracing::info!(
            "save_cookies_from_page: saved {} cookies to {}",
            serialized.len(),
            self.state_path.display()
        );
        Ok(())
    }

    /// Delete `state.json` and `session.json` (auth tokens).
    /// Called before re_auth to force a fresh login.
    pub fn clear_auth_state(&self) -> Result<()> {
        if self.state_path.exists() {
            std::fs::remove_file(&self.state_path)
                .map_err(|e| anyhow!("Cannot delete state.json: {e}"))?;
            tracing::info!("clear_auth_state: deleted {}", self.state_path.display());
        }
        if self.session_path.exists() {
            std::fs::remove_file(&self.session_path).ok();
            tracing::info!("clear_auth_state: deleted {}", self.session_path.display());
        }
        Ok(())
    }

    /// Delete the persistent Chrome profile directory so the next browser
    /// launch starts with a completely clean profile (no stored Google cookies).
    pub fn clear_chrome_profile(&self) -> Result<()> {
        let profile_dir = &config().chrome_profile_dir;
        if profile_dir.exists() {
            std::fs::remove_dir_all(profile_dir)
                .map_err(|e| anyhow!("Cannot delete Chrome profile: {e}"))?;
            std::fs::create_dir_all(profile_dir)
                .map_err(|e| anyhow!("Cannot recreate Chrome profile dir: {e}"))?;
            tracing::info!(
                "clear_chrome_profile: cleared {}",
                profile_dir.display()
            );
        }
        Ok(())
    }
}
