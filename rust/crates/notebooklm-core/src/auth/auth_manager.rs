//! Authentication Manager (Phase 5 minimal implementation)
//!
//! Handles:
//! - Reading saved browser state (Playwright storageState JSON format)
//! - Validating cookie expiry via CDP
//! - Injecting cookies into a chromiumoxide Page
//! - Session storage restore
//!
//! Full interactive login (setup_auth / re_auth) is implemented in Phase 6.

use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::network::CookieParam;
use serde::Deserialize;

use crate::config::config;

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
}
