use super::{PingResult, Provider, UsageSnapshot};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

const CLAUDE_USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage?cedar_ember=1";
const CLAUDE_BETA_HEADER: &str = "oauth-2025-04-20";

#[derive(Debug, Deserialize)]
struct ClaudeUsageResponse {
    five_hour: Option<ClaudeUsageWindow>,
    seven_day: Option<ClaudeUsageWindow>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsageWindow {
    resets_at: Option<String>,
    utilization: Option<f64>,
}

pub struct ClaudeProvider {
    custom_model: Option<String>,
}

impl ClaudeProvider {
    pub fn new(custom_model: Option<String>) -> Self {
        Self { custom_model }
    }

    fn resolve_claude_binary() -> PathBuf {
        if let Some(home) = dirs::home_dir() {
            let local_bin = home.join(".local/bin/claude");
            if local_bin.exists() {
                return local_bin;
            }
        }
        PathBuf::from("claude")
    }

    async fn get_access_token() -> Option<String> {
        // 1. Try macOS Keychain
        #[cfg(target_os = "macos")]
        {
            let output = Command::new("security")
                .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
                .output()
                .await;

            if let Ok(out) = output {
                if out.status.success() {
                    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !text.is_empty() {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(token) = json
                                .get("claudeAiOauth")
                                .and_then(|o| o.get("accessToken"))
                                .and_then(|v| v.as_str())
                            {
                                return Some(token.to_string());
                            }
                            if let Some(token) = json.get("accessToken").and_then(|v| v.as_str()) {
                                return Some(token.to_string());
                            }
                        }
                    }
                }
            }
        }

        // 2. Try file fallback (~/.claude/.credentials.json)
        if let Some(home) = dirs::home_dir() {
            let cred_path = home.join(".claude").join(".credentials.json");
            if cred_path.exists() {
                if let Ok(content) = fs::read_to_string(cred_path) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(token) = json
                            .get("claudeAiOauth")
                            .and_then(|o| o.get("accessToken"))
                            .and_then(|v| v.as_str())
                        {
                            return Some(token.to_string());
                        }
                        if let Some(token) = json.get("accessToken").and_then(|v| v.as_str()) {
                            return Some(token.to_string());
                        }
                    }
                }
            }
        }

        None
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    async fn is_available(&self) -> bool {
        let bin = Self::resolve_claude_binary();
        if bin.is_absolute() && bin.exists() {
            true
        } else {
            super::is_binary_available("claude")
        }
    }

    async fn fetch_usage(&self) -> Result<UsageSnapshot> {
        let token = Self::get_access_token().await;
        if let Some(tok) = token {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()?;

            let resp = client
                .get(CLAUDE_USAGE_ENDPOINT)
                .header("Authorization", format!("Bearer {}", tok))
                .header("anthropic-beta", CLAUDE_BETA_HEADER)
                .header("User-Agent", "claude-cli/external")
                .header("Accept", "application/json")
                .send()
                .await
                .context("Failed to query Claude usage endpoint")?;

            if resp.status().is_success() {
                let data: ClaudeUsageResponse = resp.json().await?;
                let now = Utc::now();
                let mut is_active = false;
                let mut resets_at = None;
                let mut used_percent = None;

                if let Some(five_h) = data.five_hour {
                    if let Some(u) = five_h.utilization {
                        used_percent = Some((u * 100.0).clamp(0.0, 100.0));
                    }
                    if let Some(rt_str) = five_h.resets_at {
                        if let Ok(parsed) = DateTime::parse_from_rfc3339(&rt_str) {
                            let utc = parsed.with_timezone(&Utc);
                            if utc > now {
                                resets_at = Some(utc);
                                is_active = true;
                            }
                        }
                    }
                }

                let mut secondary_resets_at = None;
                let mut secondary_used_percent = None;

                if let Some(sev_d) = data.seven_day {
                    if let Some(u) = sev_d.utilization {
                        secondary_used_percent = Some((u * 100.0).clamp(0.0, 100.0));
                    }
                    if let Some(rt_str) = sev_d.resets_at {
                        if let Ok(parsed) = DateTime::parse_from_rfc3339(&rt_str) {
                            secondary_resets_at = Some(parsed.with_timezone(&Utc));
                        }
                    }
                }

                let summary = if is_active {
                    if let Some(reset_time) = resets_at {
                        let remaining = (reset_time - now).num_seconds();
                        let hours = remaining / 3600;
                        let mins = (remaining % 3600) / 60;
                        let secs = remaining % 60;
                        format!(
                            "5h window active ({}% used, resets in {:02}h {:02}m {:02}s)",
                            used_percent.unwrap_or(0.0).round(),
                            hours,
                            mins,
                            secs
                        )
                    } else {
                        "5h window active".to_string()
                    }
                } else {
                    "5h window is free (ready for ping)".to_string()
                };

                return Ok(UsageSnapshot {
                    is_active,
                    resets_at,
                    used_percent,
                    secondary_resets_at,
                    secondary_used_percent,
                    summary,
                });
            }
        }

        // If direct usage endpoint fails or OAuth token is not accessible, return fallback
        Ok(UsageSnapshot {
            is_active: false,
            resets_at: None,
            used_percent: None,
            secondary_resets_at: None,
            secondary_used_percent: None,
            summary: "Claude Code CLI available (limits managed via CLI triggers)".to_string(),
        })
    }

    async fn trigger_ping(&self, dry_run: bool) -> Result<PingResult> {
        let bin = Self::resolve_claude_binary();
        let mut args = vec![
            "-p".to_string(),
            "ok".to_string(),
            "--no-session-persistence".to_string(),
            "--tools".to_string(),
            "".to_string(),
            "--output-format".to_string(),
            "json".to_string(),
        ];

        if let Some(model) = &self.custom_model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        let cmd_display = format!("{} {}", bin.display(), args.join(" "));

        if dry_run {
            return Ok(PingResult {
                provider: self.id().to_string(),
                command: cmd_display,
                success: true,
                message: "DRY RUN - command would be executed".to_string(),
            });
        }

        let sandbox = super::get_sandbox_dir();
        let output = Command::new(&bin)
            .current_dir(&sandbox)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("Failed to run {}", bin.display()))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("claude ping failed: {}", err.trim()));
        }

        Ok(PingResult {
            provider: self.id().to_string(),
            command: cmd_display,
            success: true,
            message: "New 5h window successfully started".to_string(),
        })
    }

    async fn is_active_task_running(&self) -> Result<bool> {
        Ok(false)
    }
}
