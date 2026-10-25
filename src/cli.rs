use crate::config::Config;
use crate::daemon::DaemonManager;
use crate::provider::{codex::CodexProvider, enabled_providers, Provider};
use crate::reset_credits::ResetCreditInventory;
use crate::scheduler::Scheduler;
use crate::state::AppState;
use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::io::Write;

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

    /// List available Codex banked resets, with live expiry countdowns
    Resets,

    /// Trigger a synthetic ping immediately for enabled providers
    Ping {
        /// Enabled provider to ping: codex, antigravity, claude, or all
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

    /// Update alp to the latest version from GitHub
    Update,
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

    let is_daemon_or_watch = matches!(
        cli.command,
        Some(Commands::Watch { .. }) | Some(Commands::Daemon { .. })
    );

    // If it's a CLI command, check for updates concurrently in background
    let update_task = if !is_daemon_or_watch && !matches!(cli.command, Some(Commands::Update)) {
        Some(tokio::spawn(crate::update::check_for_updates()))
    } else {
        None
    };

    let result = match cli.command.unwrap_or(Commands::Status { refresh: false }) {
        Commands::Status { refresh } => handle_status(&config, refresh).await,
        Commands::Resets => handle_resets(&config).await,
        Commands::Ping { provider, dry_run } => handle_ping(&config, &provider, dry_run).await,
        Commands::Watch { dry_run } => {
            let scheduler = Scheduler::new(config);
            scheduler.run_loop(dry_run).await
        }
        Commands::Daemon { action } => match action {
            DaemonAction::Install => DaemonManager::install(),
            DaemonAction::Uninstall => DaemonManager::uninstall(),
            DaemonAction::Start => DaemonManager::start(),
            DaemonAction::Stop => DaemonManager::stop(),
            DaemonAction::Status => DaemonManager::status(),
        },
        Commands::Config { action } => {
            match action.as_str() {
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
            }
            Ok(())
        }
        Commands::Update => crate::update::handle_update().await,
    };

    // If an update is available, print the notice
    if let Some(task) = update_task {
        if let Ok(Some(latest_ver)) = task.await {
            crate::update::print_update_notice(&latest_ver);
        }
    }

    result
}

async fn handle_resets(config: &Config) -> Result<()> {
    let inventory = CodexProvider::new(config.codex_model.clone())
        .fetch_reset_credits()
        .await?
        .context("Codex banked reset inventory is unavailable")?;
    render_resets(&mut std::io::stdout().lock(), inventory, Utc::now())?;
    Ok(())
}

fn render_resets(
    output: &mut impl Write,
    mut inventory: ResetCreditInventory,
    now: DateTime<Utc>,
) -> std::io::Result<()> {
    inventory.credits.retain(|credit| {
        credit.expires_at.map_or(true, |expiry| expiry > now)
    });
    inventory.credits.sort_unstable_by_key(|credit| {
        (credit.expires_at.is_none(), credit.expires_at)
    });
    writeln!(
        output,
        "{} — {} available (live)",
        "Codex banked resets".bold(),
        inventory.credits.len()
    )?;
    if inventory.credits.is_empty() {
        writeln!(output, "\nNo banked resets are currently available.")?;
        return Ok(());
    }
    for (index, credit) in inventory.credits.iter().enumerate() {
        writeln!(output, "\n{}. {}", index + 1, credit.title.bold())?;
        if let Some(expiry) = credit.expires_at {
            let seconds = (expiry - now).num_seconds();
            let remaining = reset_time_left(seconds);
            let remaining = if seconds <= 3_600 {
                remaining.red().bold()
            } else if seconds <= 10_800 {
                remaining.yellow().bold()
            } else {
                remaining.normal()
            };
            writeln!(output, "   Time left:  {}", remaining)?;
            writeln!(
                output,
                "   Expires:    {}",
                expiry.with_timezone(&Local).format("%a %d %b %Y, %H:%M:%S %:z")
            )?;
        } else {
            writeln!(output, "   Time left:  Does not expire")?;
        }
    }
    Ok(())
}

fn reset_time_left(seconds: i64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds / 3_600) % 24;
    let minutes = (seconds / 60) % 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, minutes)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else if seconds >= 60 {
        format!("{}m {}s", minutes, seconds % 60)
    } else if seconds > 0 {
        format!("{}s", seconds)
    } else {
        "<1s".to_owned()
    }
}
fn print_titlecard(_state: &AppState) {
    let line1_right = format!(
        "{} {}",
        "AI Limit Ping".bold(),
        format!("{} (alp)", env!("CARGO_PKG_VERSION")).dimmed()
    );
    let daemon_status = DaemonManager::status_line();

    let fg_c = |r: u8, g: u8, b: u8| format!("\x1b[38;2;{};{};{}m", r, g, b);
    let bg_c = |r: u8, g: u8, b: u8| format!("\x1b[48;2;{};{};{}m", r, g, b);
    let rst = "\x1b[0m";

    // Wooden Hourglass with Golden Sand (Wide Plinth design):
    // 10 columns x 10 rows (5 terminal lines)
    let mut grid: [[Option<(u8, u8, u8)>; 10]; 10] = [[None; 10]; 10];

    // Row 0: Top cap (0..10)
    for x in 0..10 {
        let t = (x as f32 - 4.5).abs() / 4.5;
        grid[0][x] = Some((
            (220.0 - 70.0 * t) as u8,
            (150.0 - 60.0 * t) as u8,
            (75.0 - 35.0 * t) as u8,
        ));
    }
    // Row 1: Top wood trim (1..9)
    for x in 1..9 {
        let t = (x as f32 - 4.5).abs() / 3.5;
        grid[1][x] = Some((
            (180.0 - 55.0 * t) as u8,
            (115.0 - 45.0 * t) as u8,
            (55.0 - 28.0 * t) as u8,
        ));
    }

    // Upper glass bulb:
    // Row 2: x=1 to 8:
    grid[2][1] = Some((200, 230, 245)); // glass highlight
    for x in 2..8 {
        let t = (x as f32 - 4.5).abs() / 2.5;
        grid[2][x] = Some((
            (255.0 - 20.0 * t) as u8,
            (215.0 - 35.0 * t) as u8,
            (75.0 - 25.0 * t) as u8,
        ));
    }
    grid[2][8] = Some((130, 175, 200));

    // Row 3: x=2 to 7:
    grid[3][2] = Some((210, 235, 250));
    for x in 3..7 {
        let t = (x as f32 - 4.5).abs() / 1.5;
        grid[3][x] = Some((
            (255.0 - 25.0 * t) as u8,
            (225.0 - 40.0 * t) as u8,
            (85.0 - 30.0 * t) as u8,
        ));
    }
    grid[3][7] = Some((130, 175, 200));

    // Row 4: Neck (x=3 to 6)
    grid[4][3] = Some((190, 225, 245));
    grid[4][4] = Some((255, 245, 140)); // bright trickling sand
    grid[4][5] = Some((250, 210, 70));
    grid[4][6] = Some((120, 165, 190));

    // Row 5: Neck lower (x=3 to 6):
    grid[5][3] = Some((190, 225, 245));
    grid[5][4] = Some((255, 245, 140));
    grid[5][5] = Some((250, 210, 70));
    grid[5][6] = Some((120, 165, 190));

    // Row 6: Lower glass opening (x=2 to 7):
    grid[6][2] = Some((210, 235, 250));
    // empty air cavity at x=3
    grid[6][4] = Some((255, 240, 120)); // sand landing peak
    grid[6][5] = Some((245, 195, 60));
    // empty air cavity at x=6
    grid[6][7] = Some((130, 175, 200));

    // Row 7: Lower glass sand mound (x=1 to 8):
    grid[7][1] = Some((200, 230, 245));
    for x in 2..8 {
        let t = (x as f32 - 4.5).abs() / 2.5;
        grid[7][x] = Some((
            (250.0 - 35.0 * t) as u8,
            (205.0 - 50.0 * t) as u8,
            (70.0 - 25.0 * t) as u8,
        ));
    }
    grid[7][8] = Some((130, 175, 200));

    // Row 8: Bottom wood trim (1..9)
    for x in 1..9 {
        let t = (x as f32 - 4.5).abs() / 3.5;
        grid[8][x] = Some((
            (180.0 - 55.0 * t) as u8,
            (115.0 - 45.0 * t) as u8,
            (55.0 - 28.0 * t) as u8,
        ));
    }
    // Row 9: Bottom wood base (0..10)
    for x in 0..10 {
        let t = (x as f32 - 4.5).abs() / 4.5;
        grid[9][x] = Some((
            (220.0 - 70.0 * t) as u8,
            (150.0 - 60.0 * t) as u8,
            (75.0 - 35.0 * t) as u8,
        ));
    }

    let mut logo = Vec::with_capacity(5);
    for line_idx in 0..5 {
        let y_top = line_idx * 2;
        let y_bot = line_idx * 2 + 1;
        let row_t = &grid[y_top];
        let row_b = &grid[y_bot];

        let mut row_str = String::with_capacity(10 * 25);
        for x in 0..10 {
            let ct = row_t[x];
            let cb = row_b[x];
            match (ct, cb) {
                (Some((rt, gt, bt)), Some((rb, gb, bb))) => {
                    row_str.push_str(&format!("{}{}\u{2580}{}", fg_c(rt, gt, bt), bg_c(rb, gb, bb), rst));
                }
                (Some((rt, gt, bt)), None) => {
                    row_str.push_str(&format!("{}\u{2580}{}", fg_c(rt, gt, bt), rst));
                }
                (None, Some((rb, gb, bb))) => {
                    row_str.push_str(&format!("{}\u{2584}{}", fg_c(rb, gb, bb), rst));
                }
                (None, None) => {
                    row_str.push(' ');
                }
            }
        }
        logo.push(row_str);
    }

    // Centered vertically relative to the 5 logo lines (lines 1 and 2)
    let info = [
        "",
        &line1_right,
        &daemon_status,
        "",
        "",
    ];

    println!();
    for (l, i) in logo.iter().zip(info.iter()) {
        if i.is_empty() {
            println!("{}", l);
        } else {
            println!("{}    {}", l, i);
        }
    }
    println!();
}

async fn handle_status(config: &Config, refresh: bool) -> Result<()> {
    let mut state = AppState::load();
    print_titlecard(&state);
    let providers = enabled_providers(config);

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
        } else if let Some(reset) = prov_state.window_resets_at {
            if reset > now {
                let diff = (reset - now).num_seconds();
                let m = (diff % 3600) / 60;
                let s = diff % 60;
                format!(
                    "{} (backoff for {:02}m {:02}s until {})",
                    "BACKOFF".yellow().bold(),
                    m,
                    s,
                    reset.with_timezone(&chrono::Local).format("%H:%M:%S")
                )
            } else {
                "WINDOW FREE (Ready for ping)".cyan().bold().to_string()
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
            let age_sec = (now - last_check).num_seconds().max(0);
            let h = age_sec / 3600;
            let m = (age_sec % 3600) / 60;
            let s = age_sec % 60;
            println!("  Last Verified:   {:02}h {:02}m {:02}s ago (locally cached)", h, m, s);
        }

        if let Some(last_ping) = prov_state.last_ping_at {
            println!(
                "  Last Ping:       {}",
                last_ping.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S")
            );
        }

        if let Some(status) = &prov_state.last_ping_status {
            if status.starts_with("Failed") {
                println!("  Last Error:      {}", status.dimmed());
            }
        }
    }

    println!("\n{}", "Tip: Run `alp status --refresh` to force an immediate online check.".dimmed());
    Ok(())
}

async fn handle_ping(config: &Config, target: &str, dry_run: bool) -> Result<()> {
    let providers = enabled_providers(config);

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

#[cfg(test)]
mod reset_listing_tests {
    use super::*;
    use crate::reset_credits::ResetCredit;
    use chrono::Duration;

    #[test]
    fn lists_only_remaining_credits_in_expiry_order() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let make_credit = |id: &str, expires_at| ResetCredit {
            id: id.to_owned(),
            title: id.to_owned(),
            expires_at,
        };
        let inventory = ResetCreditInventory {
            account_id: "listing-test".to_owned(),
            credits: vec![
                make_credit("never-expiring", None),
                make_credit("later-credit", Some(now + Duration::days(2))),
                make_credit("expired-credit", Some(now - Duration::seconds(1))),
                make_credit("expiry-boundary", Some(now)),
                make_credit("soon-credit", Some(now + Duration::minutes(50))),
            ],
        };
        let mut output = Vec::new();
        render_resets(&mut output, inventory, now).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("expired-credit"));
        assert!(!output.contains("expiry-boundary"));
        assert!(output.find("soon-credit").unwrap() < output.find("later-credit").unwrap());
        assert!(output.find("later-credit").unwrap() < output.find("never-expiring").unwrap());
    }
}
