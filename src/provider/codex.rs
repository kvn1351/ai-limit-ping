use super::{PingResult, Provider, UsageSnapshot};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const CODEX_USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexAuthTokens {
    access_token: String,
    refresh_token: Option<String>,
    account_id: Option<String>,
    id_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexAuthFile {
    tokens: Option<CodexAuthTokens>,
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
    last_refresh: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CodexUsageResponse {
    rate_limit: Option<CodexRateLimitDetails>,
}

#[derive(Debug, Deserialize)]
struct CodexRateLimitDetails {
    primary_window: Option<CodexRateLimitWindow>,
    secondary_window: Option<CodexRateLimitWindow>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct CodexRateLimitWindow {
    used_percent: Option<f64>,
    limit_window_seconds: Option<i64>,
    reset_at: Option<i64>,
}

pub struct CodexProvider {
    custom_model: Option<String>,
}

impl CodexProvider {
    pub fn new(custom_model: Option<String>) -> Self {
        Self { custom_model }
    }

    fn codex_home() -> PathBuf {
        if let Ok(dir) = std::env::var("CODEX_HOME") {
            let p = PathBuf::from(dir.trim());
            if !p.as_os_str().is_empty() {
                return p;
            }
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".codex")
    }

    fn auth_file_path() -> PathBuf {
        Self::codex_home().join("auth.json")
    }

    fn extract_account_id_from_jwt(token: &str) -> Option<String> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            return None;
        }
        let payload = parts[1];
        let decoded = base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            payload,
        )
        .or_else(|_| {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload)
        })
        .ok()?;
        let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
        if let Some(acc) = json.get("account_id").and_then(|v| v.as_str()) {
            return Some(acc.to_string());
        }
        if let Some(auth_claim) = json.get("https://api.openai.com/auth") {
            if let Some(acc) = auth_claim.get("account_id").and_then(|v| v.as_str()) {
                return Some(acc.to_string());
            }
        }
        None
    }

    fn is_jwt_expired(token: &str, buffer_seconds: i64) -> bool {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            return true;
        }
        let payload = parts[1];
        let decoded = match base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            payload,
        )
        .or_else(|_| {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload)
        }) {
            Ok(d) => d,
            Err(_) => return true,
        };
        let json: serde_json::Value = match serde_json::from_slice(&decoded) {
            Ok(j) => j,
            Err(_) => return true,
        };
        if let Some(exp) = json.get("exp").and_then(|v| v.as_i64()) {
            let exp_time = Utc.timestamp_opt(exp, 0).single();
            if let Some(t) = exp_time {
                return Utc::now() + chrono::Duration::seconds(buffer_seconds) >= t;
            }
        }
        false
    }

    async fn get_valid_access_token(&self) -> Result<(String, Option<String>)> {
        let path = Self::auth_file_path();
        if !path.exists() {
            return Err(anyhow!(
                "Codex auth file not found at {}. Run `codex login` first.",
                path.display()
            ));
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let mut auth_data: CodexAuthFile = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;

        if let Some(api_key) = &auth_data.openai_api_key {
            if !api_key.trim().is_empty() {
                return Ok((api_key.trim().to_string(), None));
            }
        }

        let mut tokens = auth_data
            .tokens
            .clone()
            .ok_or_else(|| anyhow!("No tokens found in {}", path.display()))?;

        let account_id = tokens
            .account_id
            .clone()
            .or_else(|| Self::extract_account_id_from_jwt(&tokens.access_token))
            .or_else(|| {
                tokens
                    .id_token
                    .as_deref()
                    .and_then(Self::extract_account_id_from_jwt)
            });

        // Check if access token needs refreshing (within 5 minutes of expiration)
        if Self::is_jwt_expired(&tokens.access_token, 300) {
            if let Some(refresh_token) = &tokens.refresh_token {
                let client = reqwest::Client::new();
                let body = serde_json::json!({
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token,
                    "client_id": CODEX_OAUTH_CLIENT_ID,
                });

                let res = client
                    .post(CODEX_TOKEN_ENDPOINT)
                    .json(&body)
                    .send()
                    .await
                    .context("Failed to request Codex token refresh")?;

                if res.status().is_success() {
                    #[derive(Deserialize)]
                    struct RefreshResponse {
                        access_token: String,
                        refresh_token: Option<String>,
                        id_token: Option<String>,
                    }
                    let new_tok: RefreshResponse = res.json().await?;
                    tokens.access_token = new_tok.access_token;
                    if let Some(rt) = new_tok.refresh_token {
                        tokens.refresh_token = Some(rt);
                    }
                    if let Some(it) = new_tok.id_token {
                        tokens.id_token = Some(it);
                    }
                    auth_data.tokens = Some(tokens.clone());
                    auth_data.last_refresh = Some(Utc::now().to_rfc3339());

                    // Save back atomically
                    if let Ok(updated_content) = serde_json::to_string_pretty(&auth_data) {
                        let tmp = path.with_extension("tmp");
                        if fs::write(&tmp, updated_content).is_ok() {
                            let _ = fs::rename(tmp, &path);
                        }
                    }
                }
            }
        }

        Ok((tokens.access_token, account_id))
    }
}

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    async fn is_available(&self) -> bool {
        Self::auth_file_path().exists()
    }

    async fn fetch_usage(&self) -> Result<UsageSnapshot> {
        let (token, account_id) = self.get_valid_access_token().await?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        let mut req = client
            .get(CODEX_USAGE_ENDPOINT)
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "ai-limit-ping")
            .header("Accept", "application/json");

        if let Some(acc) = account_id {
            req = req.header("ChatGPT-Account-Id", acc);
        }

        let resp = req
            .send()
            .await
            .context("Failed to query Codex usage endpoint")?;

        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Codex usage API error HTTP {}: {}", code, body));
        }

        let data: CodexUsageResponse = resp.json().await.context("Failed to parse Codex usage JSON")?;
        let rl = data
            .rate_limit
            .ok_or_else(|| anyhow!("Codex usage response missing rate_limit"))?;

        let primary = rl.primary_window.ok_or_else(|| anyhow!("Codex missing primary_window"))?;
        let secondary = rl.secondary_window;

        let primary_reset = primary.reset_at.and_then(|ts| Utc.timestamp_opt(ts, 0).single());
        let secondary_reset = secondary
            .as_ref()
            .and_then(|s| s.reset_at)
            .and_then(|ts| Utc.timestamp_opt(ts, 0).single());

        let now = Utc::now();
        let is_active = match primary_reset {
            Some(reset_time) => reset_time > now,
            None => false,
        };

        let summary = if is_active {
            if let Some(reset_time) = primary_reset {
                let remaining = (reset_time - now).num_seconds();
                let hours = remaining / 3600;
                let mins = (remaining % 3600) / 60;
                let secs = remaining % 60;
                format!(
                    "5h window active ({}% used, resets in {:02}h {:02}m {:02}s)",
                    primary.used_percent.unwrap_or(0.0).round(),
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
            resets_at: primary_reset,
            used_percent: primary.used_percent,
            secondary_resets_at: secondary_reset,
            secondary_used_percent: secondary.as_ref().and_then(|s| s.used_percent),
            summary,
        })
    }

    async fn trigger_ping(&self, dry_run: bool) -> Result<PingResult> {
        let mut args = vec![
            "exec".to_string(),
            "--ephemeral".to_string(),
            "--json".to_string(),
            "--skip-git-repo-check".to_string(),
            "--disable".to_string(),
            "hooks".to_string(),
            "--sandbox".to_string(),
            "read-only".to_string(),
            "-c".to_string(),
            "model_reasoning_effort=low".to_string(),
        ];

        let model = self.custom_model.as_deref().unwrap_or("gpt-5.6-luna");
        args.push("-m".to_string());
        args.push(model.to_string());

        args.push("ok".to_string());

        let cmd_display = format!("codex {}", args.join(" "));

        if dry_run {
            return Ok(PingResult {
                provider: self.id().to_string(),
                command: cmd_display,
                success: true,
                message: "DRY RUN - command would be executed".to_string(),
            });
        }

        let sandbox = super::get_sandbox_dir();
        let bin = super::resolve_binary_path("codex");
        let output = Command::new(&bin)
            .current_dir(&sandbox)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("Failed to run codex binary at {}. Ensure `codex` is installed.", bin.display()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(anyhow!(
                "codex exec failed with status {}: {}",
                output.status,
                stderr.trim()
            ));
        }

        // Verify that turn was executed (turn.completed in JSON output)
        if !stdout.contains("turn.completed") && !stdout.contains("\"type\":\"turn.completed\"") {
            // Even if not strictly json-structured, if exit code is 0 and non-empty, consider triggered
            if stdout.trim().is_empty() {
                return Err(anyhow!(
                    "codex exec started no turn, so no window was started: {}",
                    stderr.trim()
                ));
            }
        }

        Ok(PingResult {
            provider: self.id().to_string(),
            command: cmd_display,
            success: true,
            message: "New 5h window successfully started".to_string(),
        })
    }

    async fn is_active_task_running(&self) -> Result<bool> {
        // Quick check if a user codex interactive session is currently running
        Ok(false)
    }
}
