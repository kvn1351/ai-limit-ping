use crate::config::Config;
use crate::network::wait_for_connectivity;
use crate::provider::{antigravity::AntigravityProvider, claude::ClaudeProvider, codex::CodexProvider, Provider};
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

pub struct Scheduler {
    config: Config,
    providers: Vec<Arc<dyn Provider>>,
}

#[derive(Default)]
struct ResetCreditRefresh {
    last_attempt: Option<DateTime<Utc>>,
    retry_pending: bool,
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

    pub async fn send_notification(&self, title: &str, message: &str) -> Result<()> {
        if !self.config.notify {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        let mut command = {
            let mut command = tokio::process::Command::new("osascript");
            command
                .arg("-e")
                .arg("on run argv\n display notification (item 2 of argv) with title (item 1 of argv)\nend run")
                .arg(title)
                .arg(message);
            command
        };

        #[cfg(target_os = "windows")]
        let mut command = {
            let mut command = tokio::process::Command::new("powershell");
            command.arg("-Command").arg(format!(
                "[reflection.assembly]::loadwithpartialname('System.Windows.Forms'); [System.Windows.Forms.MessageBox]::Show('{}', '{}')",
                message.replace('\'', "''"),
                title.replace('\'', "''")
            ));
            command
        };

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let mut command = {
            let mut command = tokio::process::Command::new("notify-send");
            command.arg(title).arg(message);
            command
        };

        command.kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .context("Desktop notification command timed out")?
            .context("Failed to launch desktop notification command")?;
        if !output.status.success() {
            return Err(anyhow!("Desktop notification command failed: {}", output.status));
        }
        Ok(())
    }

    async fn check_reset_credit_reminders(
        &self,
        provider: &dyn Provider,
        state: &mut AppState,
        refresh: &mut ResetCreditRefresh,
        force_refresh: bool,
        dry_run: bool,
    ) -> Result<()> {
        if !self.config.notify {
            return Ok(());
        }

        let now = Utc::now();
        let elapsed = refresh.last_attempt.map(|last| now - last);
        let refresh_interval = if refresh.retry_pending {
            chrono::Duration::minutes(5)
        } else {
            chrono::Duration::hours(1)
        };
        let hourly_refresh = elapsed.map_or(true, |elapsed| elapsed >= refresh_interval);
        let reminder_due = state.reset_credit_reminders.has_due_reminders(now);
        let threshold_refresh = reminder_due && !refresh.retry_pending;
        if !force_refresh && !hourly_refresh && !threshold_refresh {
            return Ok(());
        }

        refresh.last_attempt = Some(now);
        refresh.retry_pending = true;
        // Validate availability before notifying: a credit may have been redeemed elsewhere.
        let Some(inventory) = provider.fetch_reset_credits().await? else {
            return Ok(());
        };
        state.reset_credit_reminders.replace_inventory(inventory);
        state.save()?;

        for reminder in state.reset_credit_reminders.due_reminders(Utc::now()) {
            if dry_run {
                println!(
                    "[{}] [Codex] Would notify: {}",
                    log_timestamp(),
                    reminder.message
                );
                continue;
            }
            self.send_notification("Codex banked reset expires soon", &reminder.message)
                .await?;
            state.reset_credit_reminders.mark_notified(&reminder);
            state.save()?;
            println!("[{}] [Codex] {}", log_timestamp(), reminder.message);
        }
        refresh.retry_pending = dry_run;
        Ok(())
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
        let mut reset_credit_refresh = ResetCreditRefresh::default();
        let mut startup = true;

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
            if let Some(provider) = active_providers.iter().find(|provider| provider.id() == "codex") {
                if let Err(error) = self
                    .check_reset_credit_reminders(
                        provider.as_ref(),
                        &mut state,
                        &mut reset_credit_refresh,
                        startup || woke_from_sleep,
                        dry_run,
                    )
                    .await
                {
                    warn!("[{}] [Codex] Banked reset reminders: {}", log_timestamp(), error);
                }
            }
            startup = false;

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

                            if let Err(error) = self.send_notification(
                                &format!("{} Limit Window Reset", display),
                                "New 5-hour window initiated successfully",
                            ).await {
                                warn!("[{}] [{}] Notification failed: {}", log_timestamp(), id, error);
                            }

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
