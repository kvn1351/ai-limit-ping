use crate::config::Config;
use crate::network::wait_for_connectivity;
use crate::provider::{antigravity::AntigravityProvider, claude::ClaudeProvider, codex::CodexProvider, Provider};
use crate::state::AppState;
use anyhow::Result;
use chrono::Utc;
use colored::Colorize;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

pub struct Scheduler {
    config: Config,
    providers: Vec<Arc<dyn Provider>>,
}

impl Scheduler {
    pub fn new(config: Config) -> Self {
        let mut providers: Vec<Arc<dyn Provider>> = Vec::new();

        if config.enabled_providers.iter().any(|p| p == "codex") {
            providers.push(Arc::new(CodexProvider::new(config.codex_model.clone())));
        }
        if config.enabled_providers.iter().any(|p| p == "antigravity" || p == "agy") {
            providers.push(Arc::new(AntigravityProvider::new(
                config.antigravity_model.clone(),
            )));
        }
        if config.enabled_providers.iter().any(|p| p == "claude") {
            providers.push(Arc::new(ClaudeProvider::new(config.claude_model.clone())));
        }

        Self { config, providers }
    }

    pub fn send_notification(&self, title: &str, message: &str) {
        if !self.config.notify {
            return;
        }

        #[cfg(target_os = "macos")]
        {
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                message.replace('"', "\\\""),
                title.replace('"', "\\\"")
            );
            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .spawn();
        }

        #[cfg(target_os = "windows")]
        {
            let script = format!(
                "[reflection.assembly]::loadwithpartialname('System.Windows.Forms'); [System.Windows.Forms.MessageBox]::Show('{}', '{}')",
                message.replace('\'', "''"),
                title.replace('\'', "''")
            );
            let _ = std::process::Command::new("powershell")
                .arg("-Command")
                .arg(&script)
                .spawn();
        }

        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("notify-send")
                .arg(title)
                .arg(message)
                .spawn();
        }
    }

    pub async fn run_loop(&self, dry_run: bool) -> Result<()> {
        info!("Starting ai-limit-ping scheduler (dry_run: {})", dry_run);

        // Filter to available providers
        let mut active_providers = Vec::new();
        for p in &self.providers {
            if p.is_available().await {
                active_providers.push(p.clone());
                println!(
                    "[{}] {} Provider {} is active",
                    log_timestamp(),
                    "✓".green(),
                    p.display_name().bold()
                );
            } else {
                println!(
                    "[{}] {} Provider {} not configured/available (skipping)",
                    log_timestamp(),
                    "!".yellow(),
                    p.display_name()
                );
            }
        }

        if active_providers.is_empty() {
            println!(
                "[{}] {}",
                log_timestamp(),
                "No providers available. Please log in to Codex, Antigravity, or Claude Code.".red()
            );
            return Ok(());
        }

        let buffer_sec = self.config.reset_buffer_seconds;
        let tick_duration = Duration::from_secs(self.config.coarse_tick_seconds.max(5));
        let mut last_tick_time = Utc::now();

        loop {
            let now = Utc::now();
            let elapsed_since_tick = (now - last_tick_time).num_seconds();
            // Detect if the machine was asleep or clock jumped significantly
            let woke_from_sleep = elapsed_since_tick > (tick_duration.as_secs() as i64 + 30);
            last_tick_time = now;

            if woke_from_sleep {
                println!(
                    "[{}] {} System resumed from sleep (clock jumped by {}s). Ensuring network connectivity...",
                    log_timestamp(),
                    "⚡".yellow().bold(),
                    elapsed_since_tick
                );
                wait_for_connectivity(Duration::from_secs(30)).await;
            }

            let mut state = AppState::load();
            let mut earliest_deadline = None;

            for provider in &active_providers {
                let id = provider.id();
                let display = provider.display_name();
                let mut prov_state = state.get_provider_cloned(id, display);

                // Check if we need to query usage:
                // 1. Resumed from sleep (cache might be obsolete)
                // 2. State has no reset deadline
                // 3. Reset deadline elapsed or is due
                // 4. Provider is inactive and needs fresh live state
                let need_query = woke_from_sleep || match prov_state.window_resets_at {
                    Some(resets_at) => {
                        let elapsed_sec = (Utc::now() - resets_at).num_seconds();
                        elapsed_sec > 60 || (!prov_state.is_active && Utc::now() >= resets_at)
                    }
                    None => true,
                };

                if need_query {
                    info!("[{}] Refreshing usage limits from provider...", id);
                    if wait_for_connectivity(Duration::from_secs(10)).await {
                        match provider.fetch_usage().await {
                            Ok(usage) => {
                                info!("[{}] {}", id, usage.summary);
                                prov_state.is_active = usage.is_active;
                                prov_state.window_resets_at = usage.resets_at;
                                prov_state.used_percent = usage.used_percent;
                                prov_state.secondary_resets_at = usage.secondary_resets_at;
                                prov_state.secondary_used_percent = usage.secondary_used_percent;
                                prov_state.last_checked_at = Some(Utc::now());
                                state.set_provider(prov_state.clone());
                                let _ = state.save();
                            }
                            Err(e) => {
                                warn!("[{}] Failed to fetch usage: {}", id, e);
                            }
                        }
                    }
                }

                // Check if window is due for a ping
                let is_due = prov_state.is_due_for_ping(buffer_sec);

                if is_due {
                    println!(
                        "[{}] {} [{}] 5h limit window has reset or is due for ping. Triggering...",
                        log_timestamp(),
                        "→".cyan().bold(),
                        display
                    );

                    // Ensure network is up (especially after sleep)
                    if !wait_for_connectivity(Duration::from_secs(30)).await {
                        warn!("[{}] Waiting for internet connection failed. Retrying in next tick.", id);
                        continue;
                    }

                    // Check if an active turn is running
                    if let Ok(true) = provider.is_active_task_running().await {
                        println!(
                            "[{}] {} [{}] Active session detected mid-turn. Waiting for user request to anchor window.",
                            log_timestamp(),
                            "⏳".yellow(),
                            display
                        );
                        continue;
                    }

                    match provider.trigger_ping(dry_run).await {
                        Ok(res) => {
                            println!(
                                "[{}] {} [{}] {}",
                                log_timestamp(),
                                "✓".green().bold(),
                                display,
                                res.message.green()
                            );
                            prov_state.last_ping_at = Some(Utc::now());
                            prov_state.last_ping_status = Some(res.message.clone());
                            prov_state.is_active = true;
                            if prov_state.window_resets_at.is_none() {
                                prov_state.window_resets_at = Some(Utc::now() + chrono::Duration::hours(5));
                            }
                            state.set_provider(prov_state.clone());
                            let _ = state.save();

                            self.send_notification(
                                &format!("{} Limit Window Reset", display),
                                "New 5-hour window initiated successfully",
                            );

                            if !dry_run {
                                // Grace period then re-read usage to record the new reset deadline
                                sleep(Duration::from_secs(12)).await;
                                if let Ok(new_usage) = provider.fetch_usage().await {
                                    prov_state.is_active = new_usage.is_active || prov_state.is_active;
                                    prov_state.window_resets_at = new_usage.resets_at.or(prov_state.window_resets_at);
                                    prov_state.used_percent = new_usage.used_percent;
                                    prov_state.last_checked_at = Some(Utc::now());
                                    state.set_provider(prov_state.clone());
                                    let _ = state.save();
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "[{}] {} [{}] Failed to ping: {}",
                                log_timestamp(),
                                "✗".red().bold(),
                                display,
                                e
                            );
                            prov_state.last_ping_at = Some(Utc::now());
                            prov_state.last_ping_status = Some(format!("Failed: {}", e));
                            if prov_state.window_resets_at.is_none() {
                                prov_state.window_resets_at = Some(Utc::now() + chrono::Duration::minutes(15));
                            }
                            state.set_provider(prov_state.clone());
                            let _ = state.save();
                        }
                    }
                }

                // Track the earliest upcoming reset deadline
                if let Some(reset) = prov_state.window_resets_at {
                    if reset > Utc::now() {
                        earliest_deadline = match earliest_deadline {
                            Some(current) if current < reset => Some(current),
                            _ => Some(reset),
                        };
                    }
                }

                state.set_provider(prov_state);
            }

            // Save state updates
            let _ = state.save();

            // Low-power sleeping:
            // Instead of busy polling, we sleep in 30s coarse ticks.
            // If the machine sleeps and wakes up, the very next tick will detect that
            // Utc::now() >= reset_at and trigger the ping immediately!
            sleep(tick_duration).await;
        }
    }
}

fn log_timestamp() -> String {
    Utc::now().with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string()
}
