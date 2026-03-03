//! NotebookLM MCP Server — Rust binary entry point

mod server;

use anyhow::Result;
use server::McpServer;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present (must happen before config() is first called)
    dotenvy::dotenv().ok();

    // Initialise tracing → stderr (stdout is reserved for MCP JSON-RPC)
    notebooklm_core::utils::logger::init_tracing();

    // Handle CLI sub-commands (e.g. `notebooklm-mcp config set profile minimal`)
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("config") {
        return cli::handle_config_command(&args[2..]);
    }

    // Initialise global config + ensure all data directories exist
    let cfg = notebooklm_core::config::config();
    notebooklm_core::config::ensure_directories();

    tracing::info!("NotebookLM MCP Server v{} (Rust)", env!("CARGO_PKG_VERSION"));
    tracing::info!("Data dir:     {}", cfg.data_dir.display());
    tracing::info!("Config dir:   {}", cfg.config_dir.display());
    tracing::info!("Headless:     {}", cfg.headless);
    tracing::info!("Max sessions: {}", cfg.max_sessions);

    // Build and start the MCP server (JSON-RPC over stdio)
    let server = McpServer::new()?;
    server.serve().await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// CLI sub-command handler
// ---------------------------------------------------------------------------

mod cli {
    use anyhow::Result;
    use notebooklm_core::utils::settings_manager::{ProfileName, SettingsManager, SettingsPatch};

    pub fn handle_config_command(args: &[String]) -> Result<()> {
        let mgr = SettingsManager::new();

        match args.first().map(String::as_str) {
            Some("get") => {
                let s = mgr.get_effective_settings();
                println!("{}", serde_json::to_string_pretty(&s)?);
            }
            Some("set") => match (args.get(1).map(String::as_str), args.get(2)) {
                (Some("profile"), Some(name)) => {
                    let profile = match name.as_str() {
                        "minimal" => ProfileName::Minimal,
                        "standard" => ProfileName::Standard,
                        "full" => ProfileName::Full,
                        other => {
                            eprintln!("Unknown profile '{other}'. Valid: minimal, standard, full");
                            std::process::exit(1);
                        }
                    };
                    mgr.save_settings(SettingsPatch { profile: Some(profile), ..Default::default() })?;
                    println!("Profile set to '{name}'");
                }
                (Some("disabled-tools"), Some(list)) => {
                    let tools: Vec<String> =
                        list.split(',').map(|s| s.trim().to_string()).collect();
                    mgr.save_settings(SettingsPatch {
                        disabled_tools: Some(tools),
                        ..Default::default()
                    })?;
                    println!("Disabled tools updated");
                }
                _ => {
                    eprintln!("Usage: notebooklm-mcp config set profile <minimal|standard|full>");
                    eprintln!("       notebooklm-mcp config set disabled-tools <tool1,tool2>");
                    std::process::exit(1);
                }
            },
            Some("reset") => {
                mgr.save_settings(SettingsPatch::default())?;
                println!("Settings reset to defaults");
            }
            _ => {
                eprintln!("Usage: notebooklm-mcp config <get|set|reset>");
                std::process::exit(1);
            }
        }

        Ok(())
    }
}
