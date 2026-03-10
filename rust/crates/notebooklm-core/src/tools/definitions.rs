//! MCP tool JSON-Schema definitions (Rust rewrite of src/tools/definitions/)
//!
//! Faithfully mirrors every tool name, description, and inputSchema from the
//! TypeScript sources and docs/tools.md.

use serde_json::{json, Value};

use crate::library::NotebookLibrary;

/// Build all tool definitions.
/// The `ask_question` description is dynamic (depends on the active notebook).
pub fn build_all_tools(library: &NotebookLibrary) -> Vec<Value> {
    vec![
        ask_question_tool(library),
        add_notebook_tool(),
        list_notebooks_tool(),
        get_notebook_tool(),
        select_notebook_tool(),
        update_notebook_tool(),
        remove_notebook_tool(),
        search_notebooks_tool(),
        get_library_stats_tool(),
        remove_source_tool(),
        list_sessions_tool(),
        close_session_tool(),
        reset_session_tool(),
        get_health_tool(),
        setup_auth_tool(),
        re_auth_tool(),
        cleanup_data_tool(),
    ]
}

// ---------------------------------------------------------------------------
// ask_question (dynamic description)
// ---------------------------------------------------------------------------

pub fn build_ask_question_description(library: &NotebookLibrary) -> String {
    let bt = "`";

    if let Some(active) = library.get_active_notebook() {
        let topics = active.topics.join(", ");
        let use_cases = active
            .use_cases
            .iter()
            .map(|uc| format!("  - {uc}"))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"# Conversational Research Partner (NotebookLM • Gemini 2.5 • Session RAG)

**Active Notebook:** {name}
**Content:** {description}
**Topics:** {topics}

> Auth tip: If login is required, use the prompt 'notebooklm.auth-setup' and then verify with the 'get_health' tool. If authentication later fails (e.g., expired cookies), use the prompt 'notebooklm.auth-repair'.

## What This Tool Is
- Full conversational research with Gemini (LLM) grounded on your notebook sources
- Session-based: each follow-up uses prior context for deeper, more precise answers
- Source-cited responses designed to minimize hallucinations

## When To Use
{use_cases}

## Rules (Important)
- Always prefer continuing an existing session for the same task
- If you start a new thread, create a new session and keep its session_id
- Ask clarifying questions before implementing; do not guess missing details
- If multiple notebooks could apply, propose the top 1–2 and ask which to use
- If task context changes, ask to reset the session or switch notebooks
- If authentication fails, use the prompts 'notebooklm.auth-repair' (or 'notebooklm.auth-setup') and verify with 'get_health'
- After every NotebookLM answer: pause, compare with the user's goal, and only respond if you are 100% sure the information is complete. Otherwise, plan the next NotebookLM question in the same session.

## Session Flow (Recommended)
{bt}{bt}{bt}javascript
// 1) Start broad (no session_id → creates one)
ask_question({{ question: "Give me an overview of [topic]" }})
// ← Save: result.session_id

// 2) Go specific (same session)
ask_question({{ question: "Key APIs/methods?", session_id }})

// 3) Cover pitfalls (same session)
ask_question({{ question: "Common edge cases + gotchas?", session_id }})

// 4) Ask for production example (same session)
ask_question({{ question: "Show a production-ready example", session_id }})
{bt}{bt}{bt}

## Automatic Multi-Pass Strategy (Host-driven)
- Simple prompts return once-and-done answers.
- For complex prompts, the host should issue follow-up calls:
  1. Implementation plan (APIs, dependencies, configuration, authentication).
  2. Pitfalls, gaps, validation steps, missing prerequisites.
- Keep the same session_id for all follow-ups, review NotebookLM's answer, and ask more questions until the problem is fully resolved.
- Before replying to the user, double-check: do you truly have everything? If not, queue another ask_question immediately.

## Notebook Selection
- Default: active notebook ({id})
- Or set notebook_id to use a library notebook
- Or set notebook_url for ad-hoc notebooks (not in library)
- If ambiguous which notebook fits, ASK the user which to use"#,
            name = active.name,
            description = active.description,
            id = active.id,
        )
    } else {
        r#"# Conversational Research Partner (NotebookLM • Gemini 2.5 • Session RAG)

## No Active Notebook
- Visit https://notebooklm.google to create a notebook and get a share link
- Use **add_notebook** to add it to your library (explains how to get the link)
- Use **list_notebooks** to show available sources
- Use **select_notebook** to set one active

> Auth tip: If login is required, use the prompt 'notebooklm.auth-setup' and then verify with the 'get_health' tool. If authentication later fails (e.g., expired cookies), use the prompt 'notebooklm.auth-repair'.

Tip: Tell the user you can manage NotebookLM library and ask which notebook to use for the current task."#
            .to_string()
    }
}

fn ask_question_tool(library: &NotebookLibrary) -> Value {
    json!({
        "name": "ask_question",
        "description": build_ask_question_description(library),
        "inputSchema": {
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask NotebookLM"
                },
                "session_id": {
                    "type": "string",
                    "description": "Optional session ID for contextual conversations. If omitted, a new session is created."
                },
                "notebook_id": {
                    "type": "string",
                    "description": "Optional notebook ID from your library. If omitted, uses the active notebook. Use list_notebooks to see available notebooks."
                },
                "notebook_url": {
                    "type": "string",
                    "description": "Optional notebook URL (overrides notebook_id). Use this for ad-hoc queries to notebooks not in your library."
                },
                "show_browser": {
                    "type": "boolean",
                    "description": "Show browser window for debugging (simple version). For advanced control (typing speed, stealth, etc.), use browser_options instead."
                },
                "browser_options": {
                    "type": "object",
                    "description": "Optional browser behavior settings. Claude can control everything: visibility, typing speed, stealth mode, timeouts. Useful for debugging or fine-tuning.",
                    "properties": {
                        "show": { "type": "boolean", "description": "Show browser window (default: from ENV or false)" },
                        "headless": { "type": "boolean", "description": "Run browser in headless mode (default: true)" },
                        "timeout_ms": { "type": "number", "description": "Browser operation timeout in milliseconds (default: 30000)" },
                        "stealth": {
                            "type": "object",
                            "description": "Human-like behavior settings to avoid detection",
                            "properties": {
                                "enabled": { "type": "boolean", "description": "Master switch for all stealth features (default: true)" },
                                "random_delays": { "type": "boolean", "description": "Random delays between actions (default: true)" },
                                "human_typing": { "type": "boolean", "description": "Human-like typing patterns (default: true)" },
                                "mouse_movements": { "type": "boolean", "description": "Realistic mouse movements (default: true)" },
                                "typing_wpm_min": { "type": "number", "description": "Minimum typing speed in WPM (default: 160)" },
                                "typing_wpm_max": { "type": "number", "description": "Maximum typing speed in WPM (default: 240)" },
                                "delay_min_ms": { "type": "number", "description": "Minimum delay between actions in ms (default: 100)" },
                                "delay_max_ms": { "type": "number", "description": "Maximum delay between actions in ms (default: 400)" }
                            }
                        },
                        "viewport": {
                            "type": "object",
                            "description": "Browser viewport size",
                            "properties": {
                                "width": { "type": "number", "description": "Viewport width in pixels (default: 1920)" },
                                "height": { "type": "number", "description": "Viewport height in pixels (default: 1080)" }
                            }
                        }
                    }
                }
            },
            "required": ["question"]
        }
    })
}

// ---------------------------------------------------------------------------
// Notebook management tools
// ---------------------------------------------------------------------------

fn add_notebook_tool() -> Value {
    json!({
        "name": "add_notebook",
        "description": "PERMISSION REQUIRED — Only when user explicitly asks to add a notebook.\n\n## Conversation Workflow (Mandatory)\nWhen the user says: \"I have a NotebookLM with X\"\n\n1) Ask URL: \"What is the NotebookLM URL?\"\n2) Ask content: \"What knowledge is inside?\" (1–2 sentences)\n3) Ask topics: \"Which topics does it cover?\" (3–5)\n4) Ask use cases: \"When should we consult it?\"\n5) Propose metadata and confirm:\n   - Name: [suggested]\n   - Description: [from user]\n   - Topics: [list]\n   - Use cases: [list]\n   \"Add it to your library now?\"\n6) Only after explicit \"Yes\" → call this tool\n\n## Rules\n- Do not add without user permission\n- Do not guess metadata — ask concisely\n- Confirm summary before calling the tool\n\n## How to Get a NotebookLM Share Link\n\nVisit https://notebooklm.google/ → Login (free: 100 notebooks, 50 sources each, 500k words, 50 daily queries)\n1) Click \"+ New\" (top right) → Upload sources (docs, knowledge)\n2) Click \"Share\" (top right) → Select \"Anyone with the link\"\n3) Click \"Copy link\" (bottom left) → Give this link to Claude\n\n(Upgraded: Google AI Pro/Ultra gives 5x higher limits)",
        "inputSchema": {
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The NotebookLM notebook URL" },
                "name": { "type": "string", "description": "Display name for the notebook (e.g., 'n8n Documentation')" },
                "description": { "type": "string", "description": "What knowledge/content is in this notebook" },
                "topics": { "type": "array", "items": { "type": "string" }, "description": "Topics covered in this notebook" },
                "content_types": { "type": "array", "items": { "type": "string" }, "description": "Types of content (e.g., ['documentation', 'examples', 'best practices'])" },
                "use_cases": { "type": "array", "items": { "type": "string" }, "description": "When should Claude use this notebook" },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tags for organization" }
            },
            "required": ["url", "name", "description", "topics"]
        }
    })
}

fn list_notebooks_tool() -> Value {
    json!({
        "name": "list_notebooks",
        "description": "List all library notebooks with metadata (name, topics, use cases, URL). Use this to present options, then ask which notebook to use for the task.",
        "inputSchema": { "type": "object", "properties": {} }
    })
}

fn get_notebook_tool() -> Value {
    json!({
        "name": "get_notebook",
        "description": "Get detailed information about a specific notebook by ID",
        "inputSchema": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "The notebook ID" }
            },
            "required": ["id"]
        }
    })
}

fn select_notebook_tool() -> Value {
    json!({
        "name": "select_notebook",
        "description": "Set a notebook as the active default (used when ask_question has no notebook_id).\n\n## When To Use\n- User switches context: \"Let's work on React now\"\n- User asks explicitly to activate a notebook\n- Obvious task change requires another notebook\n\n## Auto-Switching\n- Safe to auto-switch if the context is clear and you announce it:\n  \"Switching to React notebook for this task...\"\n- If ambiguous, ask: \"Switch to [notebook] for this task?\"",
        "inputSchema": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "The notebook ID to activate" }
            },
            "required": ["id"]
        }
    })
}

fn update_notebook_tool() -> Value {
    json!({
        "name": "update_notebook",
        "description": "Update notebook metadata based on user intent.\n\n## Pattern\n1) Identify target notebook and fields (topics, description, use_cases, tags, url)\n2) Propose the exact change back to the user\n3) After explicit confirmation, call this tool\n\nTip: You may update multiple fields at once if requested.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "The notebook ID to update" },
                "name": { "type": "string", "description": "New display name" },
                "description": { "type": "string", "description": "New description" },
                "topics": { "type": "array", "items": { "type": "string" }, "description": "New topics list" },
                "content_types": { "type": "array", "items": { "type": "string" }, "description": "New content types" },
                "use_cases": { "type": "array", "items": { "type": "string" }, "description": "New use cases" },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "New tags" },
                "url": { "type": "string", "description": "New notebook URL" }
            },
            "required": ["id"]
        }
    })
}

fn remove_notebook_tool() -> Value {
    json!({
        "name": "remove_notebook",
        "description": "Dangerous — requires explicit user confirmation.\n\n## Confirmation Workflow\n1) User requests removal (\"Remove the React notebook\")\n2) Look up full name to confirm\n3) Ask: \"Remove '[notebook_name]' from your library? (Does not delete the actual NotebookLM notebook)\"\n4) Only on explicit \"Yes\" → call remove_notebook\n\nNever remove without permission or based on assumptions.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "The notebook ID to remove" }
            },
            "required": ["id"]
        }
    })
}

fn search_notebooks_tool() -> Value {
    json!({
        "name": "search_notebooks",
        "description": "Search library by query (name, description, topics, tags). Use to propose relevant notebooks for the task and then ask which to use.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" }
            },
            "required": ["query"]
        }
    })
}

fn get_library_stats_tool() -> Value {
    json!({
        "name": "get_library_stats",
        "description": "Get statistics about your notebook library (total notebooks, usage, etc.)",
        "inputSchema": { "type": "object", "properties": {} }
    })
}

// ---------------------------------------------------------------------------
// Source management tools
// ---------------------------------------------------------------------------

fn remove_source_tool() -> Value {
    json!({
        "name": "remove_source",
        "description": "Remove a source from a specific NotebookLM notebook by its document title.\n\n## Workflow\n1) Identify the target notebook (notebook_id or notebook_url)\n2) Provide the exact document title as it appears in the sources panel\n3) The tool opens the notebook in a browser, finds the source, opens its context menu, and clicks Remove\n\n## Notes\n- The document_name must match the source title exactly (case-insensitive)\n- Open the notebook manually to confirm the exact title",
        "inputSchema": {
            "type": "object",
            "properties": {
                "document_name": {
                    "type": "string",
                    "description": "Exact title of the source to remove, as shown in the NotebookLM sources panel (case-insensitive)"
                },
                "notebook_id": {
                    "type": "string",
                    "description": "ID of the target notebook in your library. If omitted, uses the active notebook."
                },
                "notebook_url": {
                    "type": "string",
                    "description": "Direct NotebookLM notebook URL (overrides notebook_id). Use for notebooks not in your library."
                }
            },
            "required": ["document_name"]
        }
    })
}

// ---------------------------------------------------------------------------
// Session management tools
// ---------------------------------------------------------------------------

fn list_sessions_tool() -> Value {
    json!({
        "name": "list_sessions",
        "description": "List all active sessions with stats (age, message count, last activity). Use to continue the most relevant session instead of starting from scratch.",
        "inputSchema": { "type": "object", "properties": {} }
    })
}

fn close_session_tool() -> Value {
    json!({
        "name": "close_session",
        "description": "Close a specific session by session ID. Ask before closing if the user might still need it.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string", "description": "The session ID to close" }
            },
            "required": ["session_id"]
        }
    })
}

fn reset_session_tool() -> Value {
    json!({
        "name": "reset_session",
        "description": "Reset a session's chat history (keep same session ID). Use for a clean slate when the task changes; ask the user before resetting.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": { "type": "string", "description": "The session ID to reset" }
            },
            "required": ["session_id"]
        }
    })
}

// ---------------------------------------------------------------------------
// System tools
// ---------------------------------------------------------------------------

fn get_health_tool() -> Value {
    json!({
        "name": "get_health",
        "description": "Get server health status including authentication state, active sessions, and configuration. Use this to verify the server is ready before starting research workflows.\n\nIf authenticated=false and having persistent issues:\nConsider running cleanup_data(preserve_library=true) + setup_auth for fresh start with clean browser session.",
        "inputSchema": { "type": "object", "properties": {} }
    })
}

fn setup_auth_tool() -> Value {
    json!({
        "name": "setup_auth",
        "description": "Google authentication for NotebookLM access - opens a browser window for manual login to your Google account. Returns immediately after opening the browser. You have up to 10 minutes to complete the login. Use 'get_health' tool afterwards to verify authentication was saved successfully.\n\nTROUBLESHOOTING for persistent auth issues:\nIf setup_auth fails or you encounter browser/session issues:\n1. Ask user to close ALL Chrome/Chromium instances\n2. Run cleanup_data(confirm=true, preserve_library=true) to clean old data\n3. Run setup_auth again for fresh start",
        "inputSchema": {
            "type": "object",
            "properties": {
                "show_browser": { "type": "boolean", "description": "Show browser window (default: true for setup)" },
                "browser_options": {
                    "type": "object",
                    "description": "Optional browser settings.",
                    "properties": {
                        "show": { "type": "boolean" },
                        "headless": { "type": "boolean" },
                        "timeout_ms": { "type": "number" }
                    }
                }
            }
        }
    })
}

fn re_auth_tool() -> Value {
    json!({
        "name": "re_auth",
        "description": "Switch to a different Google account or re-authenticate. Use this when:\n- NotebookLM rate limit is reached (50 queries/day for free accounts)\n- You want to switch to a different Google account\n- Authentication is broken and needs a fresh start\n\nThis will:\n1. Close all active browser sessions\n2. Delete all saved authentication data (cookies, Chrome profile)\n3. Open browser for fresh Google login\n\nAfter completion, use 'get_health' to verify authentication.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "show_browser": { "type": "boolean", "description": "Show browser window (default: true for re-auth)" },
                "browser_options": {
                    "type": "object",
                    "description": "Optional browser settings.",
                    "properties": {
                        "show": { "type": "boolean" },
                        "headless": { "type": "boolean" },
                        "timeout_ms": { "type": "number" }
                    }
                }
            }
        }
    })
}

fn cleanup_data_tool() -> Value {
    json!({
        "name": "cleanup_data",
        "description": "ULTRATHINK Deep Cleanup - Scans entire system for ALL NotebookLM MCP data files across 8 categories. Always runs in deep mode, shows categorized preview before deletion.\n\n⚠️ CRITICAL: Close ALL Chrome/Chromium instances BEFORE running this tool!\n\nCategories scanned:\n1. Legacy Installation (notebooklm-mcp-nodejs)\n2. Current Installation (notebooklm-mcp)\n3. NPM/NPX Cache\n4. Claude CLI MCP Logs\n5. Temporary Backups\n6. Claude Projects Cache\n7. Editor Logs (Cursor/VSCode)\n8. Trash Files\n\nLIBRARY PRESERVATION: Set preserve_library=true to keep your notebook library.json file while cleaning everything else.\n\nRECOMMENDED WORKFLOW:\n1. Ask user to close ALL Chrome/Chromium instances\n2. Run cleanup_data(confirm=false, preserve_library=true) to preview\n3. Run cleanup_data(confirm=true, preserve_library=true) to execute\n4. Run setup_auth or re_auth for fresh browser session",
        "inputSchema": {
            "type": "object",
            "properties": {
                "confirm": { "type": "boolean", "description": "Set to true only after user has reviewed the preview and explicitly confirmed." },
                "preserve_library": { "type": "boolean", "description": "Preserve library.json file during cleanup. Default: false.", "default": false }
            },
            "required": ["confirm"]
        }
    })
}
