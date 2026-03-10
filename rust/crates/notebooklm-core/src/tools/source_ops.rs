//! Browser automation for NotebookLM source management.
//!
//! Provides `browser_remove_source` to delete a source from a notebook
//! by hovering over its row in the sources panel and clicking the context
//! menu delete action.

use anyhow::{anyhow, Result};
use chromiumoxide::Page;

use crate::utils::stealth::random_delay;

// ---------------------------------------------------------------------------
// Public: remove_source
// ---------------------------------------------------------------------------

/// Remove a source from the notebook currently loaded in `page` by its document title.
///
/// DOM structure (as of 2026-03):
/// - Source items live inside `source-picker > div > div > div > div[N]`
/// - Hovering reveals a vertical 3-dots button at the start of the row
/// - Clicking it opens a CDK overlay; the first button in the panel is "Delete"
///
/// Returns `true` if the source was found and removal was initiated,
/// `false` if no source with that name was found.
///
/// # Preconditions
/// - `page` must be navigated to the notebook URL and authenticated.
pub async fn browser_remove_source(page: &Page, document_name: &str) -> Result<bool> {
    tracing::info!("source/remove: searching for source '{}'...", document_name);

    // 1. Find the source item in source-picker and click its 3-dots menu button.
    //    The button is hidden until hover; we dispatch mouseover/mouseenter first,
    //    then find and click the first button inside the same row.
    let found = find_and_open_source_menu(page, document_name).await?;
    if !found {
        tracing::warn!("  Source '{}' not found in sources panel", document_name);
        return Ok(false);
    }
    random_delay(500.0, 900.0).await;

    // 2. Click the delete/remove button from the CDK overlay menu that just opened.
    //    The overlay panel is the last `.cdk-overlay-pane` in the DOM; its first
    //    button is the delete action.
    if !click_overlay_menu_first_button(page).await? {
        return Err(anyhow!(
            "Opened context menu for '{}' but could not click the delete button. \
            NotebookLM's UI may have changed.",
            document_name
        ));
    }
    random_delay(800.0, 1500.0).await;

    // 3. Confirm deletion if a confirmation dialog appears.
    //    The dialog has two buttons (e.g. "Cancel" and "Remove") — we must click
    //    the destructive one (last button, or one whose text contains remove/delete).
    let confirm_js = r#"() => {
        // Look for a confirmation dialog button by text content
        const keywords = ['remove', 'delete', 'confirm', 'yes'];
        const candidates = Array.from(document.querySelectorAll(
            '.mat-mdc-dialog-actions button, .mat-dialog-actions button, ' +
            '[role="dialog"] button, .cdk-overlay-pane button'
        ));
        // Prefer a button whose text matches our keywords
        const destructive = candidates.find(b => {
            const t = b.textContent.trim().toLowerCase();
            return keywords.some(k => t.includes(k));
        });
        if (destructive) { destructive.click(); return { confirmed: true, text: destructive.textContent.trim().slice(0, 40) }; }
        return { confirmed: false };
    }"#;

    if let Ok(res) = page.evaluate_function(confirm_js).await {
        let val = res.value().cloned().unwrap_or_default();
        if val.get("confirmed").and_then(|v| v.as_bool()).unwrap_or(false) {
            let text = val.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
            tracing::info!("source/remove: confirmation dialog clicked '{text}'");
            random_delay(1500.0, 2500.0).await;
        }
    }

    tracing::info!("source/remove: '{}' removal initiated", document_name);
    Ok(true)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Find a source item inside `source-picker` by its visible text, dispatch hover events,
/// then click the vertical 3-dots button that appears at the start of the row.
///
/// Returns `true` if the source was found and the menu button was clicked.
async fn find_and_open_source_menu(page: &Page, name: &str) -> Result<bool> {
    let escaped = name.replace('\'', "\\'");
    let js = format!(
        r#"async () => {{
            const target = '{escaped}';
            const lc = target.toLowerCase();

            // Source-picker items: source-picker > div > div[2] > div > div[N]
            // Each item contains the file name somewhere in its text nodes.
            const container = document.querySelector('source-picker');
            if (!container) return {{ found: false, reason: 'source-picker not found' }};

            const items = Array.from(container.querySelectorAll('div[class]'))
                .filter(el => {{
                    // Pick leaf-ish divs that contain the title text
                    const txt = el.textContent.trim().toLowerCase();
                    return txt === lc || txt.startsWith(lc);
                }});

            if (items.length === 0) return {{ found: false, reason: 'no matching item' }};

            // Use the first (shallowest) matching element as the row
            const item = items[0];

            // Hover to reveal the 3-dots button
            item.dispatchEvent(new MouseEvent('mouseenter', {{ bubbles: true }}));
            item.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true }}));

            // Small pause for Angular to show the button (we'll await in Rust)
            await new Promise(r => setTimeout(r, 300));

            // Walk up until we find a parent that has a button child
            let el = item;
            for (let i = 0; i < 8; i++) {{
                el.dispatchEvent(new MouseEvent('mouseenter', {{ bubbles: true }}));
                el.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true }}));
                const btn = el.querySelector('button');
                if (btn) {{
                    btn.click();
                    return {{ found: true, clicked: btn.getAttribute('aria-label') || btn.textContent.trim().slice(0,40) }};
                }}
                if (!el.parentElement) break;
                el = el.parentElement;
            }}

            return {{ found: true, reason: 'button not found after hover' }};
        }}"#,
        escaped = escaped,
    );

    let res = page
        .evaluate_function(&js)
        .await
        .map_err(|e| anyhow!("find_and_open_source_menu JS failed: {e}"))?;

    let val = res.value().cloned().unwrap_or_default();
    tracing::debug!("find_and_open_source_menu: {val}");

    let found = val.get("found").and_then(|v| v.as_bool()).unwrap_or(false);
    let clicked = val.get("clicked").is_some();

    if !found {
        let reason = val
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        tracing::debug!("source/remove: not found — {reason}");
        return Ok(false);
    }

    if !clicked {
        // Found but menu button not yet visible — try again after a small delay
        tracing::debug!("source/remove: item found but button missing, retrying after delay...");
        random_delay(500.0, 800.0).await;
        let retry_js = format!(
            r#"() => {{
                const lc = '{}';
                const container = document.querySelector('source-picker');
                if (!container) return false;
                const items = Array.from(container.querySelectorAll('div[class]'))
                    .filter(el => el.textContent.trim().toLowerCase() === lc || el.textContent.trim().toLowerCase().startsWith(lc));
                if (!items.length) return false;
                let el = items[0];
                for (let i = 0; i < 8; i++) {{
                    const btn = el.querySelector('button');
                    if (btn) {{ btn.click(); return true; }}
                    if (!el.parentElement) break;
                    el = el.parentElement;
                }}
                return false;
            }}"#,
            escaped
        );
        if let Ok(r) = page.evaluate_function(&retry_js).await {
            if r.value().and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(true);
            }
        }
        return Err(anyhow!(
            "Source '{}' found but its 3-dots menu button could not be clicked. \
             NotebookLM's UI may have changed.",
            name
        ));
    }

    Ok(true)
}

/// Click the first button inside the most recently opened CDK overlay panel.
///
/// NotebookLM's source context menu is a Material `mat-menu` rendered in a
/// `.cdk-overlay-pane`.  The first button in that panel is always the delete action.
///
/// Returns `true` if a button was found and clicked.
async fn click_overlay_menu_first_button(page: &Page) -> Result<bool> {
    let js = r#"() => {
        // The menu panel is the last .cdk-overlay-pane that contains buttons
        const panes = Array.from(document.querySelectorAll('.cdk-overlay-pane'));
        for (let i = panes.length - 1; i >= 0; i--) {
            const btn = panes[i].querySelector('button');
            if (btn) {
                btn.click();
                return { clicked: true, text: btn.textContent.trim().slice(0, 60) };
            }
        }
        // Fallback: any visible mat-menu-panel
        const menu = document.querySelector('.mat-mdc-menu-panel, .mat-menu-panel');
        if (menu) {
            const btn = menu.querySelector('button');
            if (btn) { btn.click(); return { clicked: true, text: btn.textContent.trim().slice(0, 60) }; }
        }
        return { clicked: false };
    }"#;

    let res = page
        .evaluate_function(js)
        .await
        .map_err(|e| anyhow!("click_overlay_menu_first_button JS failed: {e}"))?;

    let val = res.value().cloned().unwrap_or_default();
    let clicked = val.get("clicked").and_then(|v| v.as_bool()).unwrap_or(false);
    if clicked {
        let text = val
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        tracing::info!("source/remove: clicked overlay menu button: '{text}'");
    }
    Ok(clicked)
}
