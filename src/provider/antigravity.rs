use super::{PingResult, Provider, UsageSnapshot};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct AgyUsageOutput {
    status: Option<String>,
    command: Option<AgyCommand>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct AgyCommand {
    name: Option<String>,
    data: Option<AgyUsageData>,
}

#[derive(Debug, Deserialize)]
struct AgyUsageData {
    groups: Option<Vec<AgyGroup>>,
}

#[derive(Debug, Deserialize)]
struct AgyGroup {
    name: Option<String>,
    buckets: Option<Vec<AgyBucket>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct AgyBucket {
    id: Option<String>,
    name: Option<String>,
    window: Option<String>,
    remaining_fraction: Option<f64>,
    reset_time: Option<String>,
}

pub struct AntigravityProvider {
    custom_model: Option<String>,
}

impl AntigravityProvider {
    pub fn new(custom_model: Option<String>) -> Self {
        Self { custom_model }
    }

    fn resolve_agy_binary() -> PathBuf {
        if let Ok(path) = std::env::var("ANTIGRAVITY_CLI_PATH") {
            let p = PathBuf::from(path.trim());
            if p.exists() {
                return p;
            }
        }
        super::resolve_binary_path("agy")
    }
}

fn parse_usage_output(stdout: &str, now: DateTime<Utc>) -> Result<UsageSnapshot> {
    let parsed: AgyUsageOutput = serde_json::from_str(stdout)
        .with_context(|| format!("Failed to parse agy /usage JSON: {}", stdout))?;

    let groups = parsed
        .command
        .and_then(|c| c.data)
        .and_then(|d| d.groups)
        .unwrap_or_default();

    let mut primary_5h_bucket: Option<AgyBucket> = None;
    let mut weekly_bucket: Option<AgyBucket> = None;

    for group in groups {
        let group_name = group.name.unwrap_or_default();
        for bucket in group.buckets.unwrap_or_default() {
            let id = bucket.id.as_deref().unwrap_or_default();
            let window = bucket.window.as_deref().unwrap_or_default();

            if id == "gemini-5h" || (window == "5h" && group_name.contains("Gemini")) {
                primary_5h_bucket = Some(bucket);
            } else if id == "gemini-weekly" || (window == "weekly" && group_name.contains("Gemini"))
            {
                weekly_bucket = Some(bucket);
            }
        }
    }

    let mut resets_at: Option<DateTime<Utc>> = None;
    let mut used_percent: Option<f64> = None;
    let mut is_active = false;

    if let Some(b) = primary_5h_bucket {
        // A future reset with 100% remaining is agy's idle sliding-window placeholder, not an active window.
        let has_usage = b
            .remaining_fraction
            .map(|remaining| remaining < 1.0)
            .unwrap_or(true);
        if let Some(rf) = b.remaining_fraction {
            used_percent = Some(((1.0 - rf) * 100.0).clamp(0.0, 100.0));
        }
        if let Some(rt_str) = b.reset_time {
            if let Ok(parsed_time) = DateTime::parse_from_rfc3339(&rt_str) {
                let utc_reset = parsed_time.with_timezone(&Utc);
                if utc_reset > now && has_usage {
                    resets_at = Some(utc_reset);
                    is_active = true;
                }
            }
        }
    }

    let mut secondary_resets_at = None;
    let mut secondary_used_percent = None;

    if let Some(b) = weekly_bucket {
        if let Some(rf) = b.remaining_fraction {
            secondary_used_percent = Some(((1.0 - rf) * 100.0).clamp(0.0, 100.0));
        }
        if let Some(rt_str) = b.reset_time {
            if let Ok(parsed_time) = DateTime::parse_from_rfc3339(&rt_str) {
                secondary_resets_at = Some(parsed_time.with_timezone(&Utc));
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

    Ok(UsageSnapshot {
        is_active,
        resets_at,
        used_percent,
        secondary_resets_at,
        secondary_used_percent,
        summary,
    })
}

#[async_trait]
impl Provider for AntigravityProvider {
    fn id(&self) -> &'static str {
        "antigravity"
    }

    fn display_name(&self) -> &'static str {
        "Antigravity (agy)"
    }

    async fn is_available(&self) -> bool {
        let bin = Self::resolve_agy_binary();
        if bin.is_absolute() && bin.exists() {
            true
        } else {
            super::is_binary_available("agy")
        }
    }

    async fn fetch_usage(&self) -> Result<UsageSnapshot> {
        let bin = Self::resolve_agy_binary();
        let sandbox = super::get_sandbox_dir();
        let output = Command::new(&bin)
            .current_dir(&sandbox)
            .args([
                "-p",
                "/usage",
                "--output-format",
                "json",
                "--print-timeout",
                "30s",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("Failed to invoke {}", bin.display()))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("agy /usage failed: {}", err.trim()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_usage_output(&stdout, Utc::now())
    }
    async fn trigger_ping(&self, dry_run: bool) -> Result<PingResult> {
        let bin = Self::resolve_agy_binary();
        let model = self
            .custom_model
            .as_deref()
            .unwrap_or("gemini-3.8-flash-low");
        let args = vec![
            "-p".to_string(),
            "ok".to_string(),
            "--sandbox".to_string(),
            "--output-format".to_string(),
            "json".to_string(),
            "--effort".to_string(),
            "low".to_string(),
            "--model".to_string(),
            model.to_string(),
        ];
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
            return Err(anyhow!("agy ping failed: {}", err.trim()));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_time(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn idle_five_hour_bucket_is_not_active() {
        let stdout = r#"{
            "command": {
                "data": {
                    "groups": [{
                        "name": "Gemini Models",
                        "buckets": [{
                            "id": "gemini-5h",
                            "window": "5h",
                            "remaining_fraction": 1.0,
                            "reset_time": "2026-10-12T05:00:00Z"
                        }]
                    }]
                }
            }
        }"#;

        let snapshot = parse_usage_output(stdout, test_time("2026-10-12T00:00:00Z")).unwrap();

        assert_eq!(snapshot.used_percent, Some(0.0));
        assert!(!snapshot.is_active);
        assert_eq!(snapshot.resets_at, None);
    }

    #[test]
    fn consumed_five_hour_bucket_is_active() {
        let stdout = r#"{
            "command": {
                "data": {
                    "groups": [{
                        "name": "Gemini Models",
                        "buckets": [{
                            "id": "gemini-5h",
                            "window": "5h",
                            "remaining_fraction": 0.75,
                            "reset_time": "2026-10-12T05:00:00Z"
                        }]
                    }]
                }
            }
        }"#;

        let snapshot = parse_usage_output(stdout, test_time("2026-10-12T00:00:00Z")).unwrap();

        assert_eq!(snapshot.used_percent, Some(25.0));
        assert!(snapshot.is_active);
        assert_eq!(snapshot.resets_at, Some(test_time("2026-10-12T05:00:00Z")));
    }
}
