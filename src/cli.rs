use crate::config::Config;
use crate::daemon::DaemonManager;
use crate::provider::{antigravity::AntigravityProvider, claude::ClaudeProvider, codex::CodexProvider, Provider};
use crate::scheduler::Scheduler;
use crate::state::AppState;
use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "alp",
    about = "Near 0% CPU & RAM background daemon and CLI that keeps 5-hour rate limit windows alive for Codex, Antigravity, and Claude Code",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Show current 5-hour limits, countdowns, and cached rollover state
    Status {
        /// Force an immediate network refresh from provider APIs instead of using local cache
        #[arg(short, long)]
        refresh: bool,
    },

    /// Trigger a synthetic ping immediately to restart the 5-hour window
    Ping {
        /// Provider to ping: codex, antigravity, claude, or all
        #[arg(default_value = "all")]
        provider: String,

        /// Print the command that would run without spending any quota
        #[arg(short, long)]
        dry_run: bool,
    },

    /// Run the scheduler in the foreground
    Watch {
        /// Run in dry-run mode (does not send real pings)
        #[arg(short, long)]
        dry_run: bool,
    },

    /// Manage the background daemon service (macOS launchd / Windows Task Scheduler)
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    /// Show or manage configuration settings
    Config {
        /// Action: show or init
        #[arg(default_value = "show")]
        action: String,
    },
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Install and enable the daemon service
    Install,
    /// Uninstall and remove the daemon service
    Uninstall,
    /// Start the installed daemon service
    Start,
    /// Stop the running daemon service
    Stop,
    /// Check daemon service registration and status
    Status,
}

pub async fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load_or_default();

    match cli.command.unwrap_or(Commands::Status { refresh: false }) {
        Commands::Status { refresh } => {
            handle_status(&config, refresh).await?;
        }
        Commands::Ping { provider, dry_run } => {
            handle_ping(&config, &provider, dry_run).await?;
        }
        Commands::Watch { dry_run } => {
            let scheduler = Scheduler::new(config);
            scheduler.run_loop(dry_run).await?;
        }
        Commands::Daemon { action } => match action {
            DaemonAction::Install => DaemonManager::install()?,
            DaemonAction::Uninstall => DaemonManager::uninstall()?,
            DaemonAction::Start => DaemonManager::start()?,
            DaemonAction::Stop => DaemonManager::stop()?,
            DaemonAction::Status => DaemonManager::status()?,
        },
        Commands::Config { action } => match action.as_str() {
            "show" => {
                let content = serde_json::to_string_pretty(&config)?;
                println!("Config path: {}\n", Config::config_path().display());
                println!("{}", content);
            }
            "init" => {
                config.save()?;
                println!("{} Saved default config to {}", "✓".green(), Config::config_path().display());
            }
            _ => {
                println!("Unknown config action: {}. Use 'show' or 'init'.", action);
            }
        },
    }

    Ok(())
}

async fn handle_status(config: &Config, refresh: bool) -> Result<()> {
    println!("{}", "=== AI Limit Window Status ===".bold());

    let mut state = AppState::load();
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(CodexProvider::new(config.codex_model.clone())),
        Arc::new(AntigravityProvider::new(config.antigravity_model.clone())),
        Arc::new(ClaudeProvider::new(config.claude_model.clone())),
    ];

    let now = Utc::now();

    for p in providers {
        let id = p.id();
        let display = p.display_name();

        if !p.is_available().await {
            println!(
                "\n{} {}: {}",
                "•".dimmed(),
                display.bold(),
                "Not available / not signed in".yellow()
            );
            continue;
        }

        let mut prov_state = state.get_provider_cloned(id, display);

        if refresh {
            print!("  Querying {} API... ", display);
            match p.fetch_usage().await {
                Ok(u) => {
                    println!("{}", "done".green());
                    prov_state.is_active = u.is_active;
                    prov_state.window_resets_at = u.resets_at;
                    prov_state.used_percent = u.used_percent;
                    prov_state.secondary_resets_at = u.secondary_resets_at;
                    prov_state.secondary_used_percent = u.secondary_used_percent;
                    prov_state.last_checked_at = Some(now);
                    state.set_provider(prov_state.clone());
                    let _ = state.save();
                }
                Err(e) => {
                    println!("{} ({})", "failed".red(), e);
                }
            }
        }

        let status_badge = if prov_state.is_active {
            if let Some(reset) = prov_state.window_resets_at {
                if reset > now {
                    let diff = (reset - now).num_seconds();
                    let h = diff / 3600;
                    let m = (diff % 3600) / 60;
                    let s = diff % 60;
                    format!(
                        "{} (resets in {:02}h {:02}m {:02}s at {})",
                        "ACTIVE".green().bold(),
                        h,
                        m,
                        s,
                        reset.with_timezone(&chrono::Local).format("%H:%M:%S")
                    )
                } else {
                    "RESET DUE".yellow().bold().to_string()
                }
            } else {
                "ACTIVE".green().bold().to_string()
            }
        } else {
            "WINDOW FREE (Ready for ping)".cyan().bold().to_string()
        };

        println!("\n{} {}", "•".cyan().bold(), display.bold());
        println!("  Status:          {}", status_badge);

        if let Some(used) = prov_state.used_percent {
            println!("  5h Window Used:  {:.1}%", used);
        }

        if let Some(sec_used) = prov_state.secondary_used_percent {
            println!("  Weekly Cap Used: {:.1}%", sec_used);
        }

        if let Some(last_check) = prov_state.last_checked_at {
            let age_sec = (now - last_check).num_seconds();
            println!("  Last Verified:   {}s ago (locally cached)", age_sec);
        }

        if let Some(last_ping) = prov_state.last_ping_at {
            println!(
                "  Last Ping:       {}",
                last_ping.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S")
            );
        }
    }

    println!("\n{}", "Tip: Run `alp status --refresh` to force an immediate online check.".dimmed());
    Ok(())
}

async fn handle_ping(config: &Config, target: &str, dry_run: bool) -> Result<()> {
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(CodexProvider::new(config.codex_model.clone())),
        Arc::new(AntigravityProvider::new(config.antigravity_model.clone())),
        Arc::new(ClaudeProvider::new(config.claude_model.clone())),
    ];

    let mut state = AppState::load();

    for p in providers {
        let id = p.id();
        if target != "all" && target != id && target != p.display_name().to_lowercase() {
            continue;
        }

        if !p.is_available().await {
            println!(
                "{} [{}] Not available or logged in. Skipping.",
                "!".yellow(),
                p.display_name()
            );
            continue;
        }

        println!("{} [{}] Sending headless ping...", "→".cyan(), p.display_name());
        match p.trigger_ping(dry_run).await {
            Ok(res) => {
                println!("{} [{}] {}", "✓".green().bold(), p.display_name(), res.message);
                if !dry_run {
                    let mut prov_state = state.get_provider_cloned(id, p.display_name());
                    prov_state.last_ping_at = Some(Utc::now());
                    prov_state.last_ping_status = Some(res.message);
                    state.set_provider(prov_state);
                    let _ = state.save();
                }
            }
            Err(e) => {
                println!("{} [{}] Ping failed: {}", "✗".red().bold(), p.display_name(), e);
            }
        }
    }

    Ok(())
}
