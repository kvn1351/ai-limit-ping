pub mod antigravity;
pub mod claude;
pub mod codex;

use anyhow::Result;
use chrono::{DateTime, Utc};
use crate::reset_credits::ResetCreditInventory;
use crate::config::Config;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitWindow {
    #[default]
    FiveHour,
    Weekly,
}

impl LimitWindow {
    pub fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5-hour",
            Self::Weekly => "weekly",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UsageSnapshot {
    /// Whether the primary quota window is active
    pub is_active: bool,
    /// The exact UTC time when the primary quota window will reset
    pub resets_at: Option<DateTime<Utc>>,
    /// Percentage used in the primary quota window (0.0 to 100.0)
    pub used_percent: Option<f64>,
    /// Secondary window reset time (e.g. weekly limit)
    pub secondary_resets_at: Option<DateTime<Utc>>,
    /// Secondary window percentage used
    pub secondary_used_percent: Option<f64>,
    /// Descriptive summary (e.g. "Primary 5h window: 4h 12m remaining")
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct PingResult {
    pub provider: String,
    pub command: String,
    pub success: bool,
    pub message: String,
}

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;

    fn limit_window(&self) -> LimitWindow {
        LimitWindow::FiveHour
    }

    /// CLI target that includes this provider's quota tracks.
    fn ping_target(&self) -> &'static str {
        self.id()
    }

    /// Checks if CLI tool or credentials for this provider are installed/configured
    async fn is_available(&self) -> bool;

    /// Fetches the latest usage limits and reset timestamps from the API / CLI
    async fn fetch_usage(&self) -> Result<UsageSnapshot>;

    /// Fetches banked reset credits, when supported by the provider
    async fn fetch_reset_credits(&self) -> Result<Option<ResetCreditInventory>> {
        Ok(None)
    }

    /// Sends a synthetic minimal turn to activate the relevant quota window
    async fn trigger_ping(&self, dry_run: bool) -> Result<PingResult>;

    /// Detects if an active user turn is already in progress, avoiding synthetic interference
    async fn is_active_task_running(&self) -> Result<bool>;
}

/// Selects configured providers for status, manual pings, and the scheduler.
pub fn enabled_providers(config: &Config) -> Vec<Arc<dyn Provider>> {
    let mut providers: Vec<Arc<dyn Provider>> = Vec::new();

    if config.enabled_providers.iter().any(|p| p == "codex") {
        providers.push(Arc::new(codex::CodexProvider::new(config.codex_model.clone())));
    }
    if config.enabled_providers.iter().any(|p| p == "antigravity" || p == "agy") {
        providers.push(Arc::new(antigravity::AntigravityProvider::new(
            config.antigravity_model.clone(),
        )));
        providers.push(Arc::new(antigravity::AntigravityProvider::argon()));
    }
    if config.enabled_providers.iter().any(|p| p == "claude") {
        providers.push(Arc::new(claude::ClaudeProvider::new(config.claude_model.clone())));
    }

    providers
}

/// Returns an isolated scratch directory for synthetic pings so subprocesses don't scan user files/folders
pub fn get_sandbox_dir() -> std::path::PathBuf {
    let p = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".local/share/ai-limit-ping/sandbox");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// Cross-platform helper to resolve the full executable path of a CLI tool.
/// Checks absolute paths, ~/.local/bin, /opt/homebrew/bin, /usr/local/bin, ~/.cargo/bin, and PATH.
pub fn resolve_binary_path(name: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(name);
    if path.is_absolute() && path.exists() {
        return path.to_path_buf();
    }

    if let Some(home) = dirs::home_dir() {
        let candidates = [
            home.join(".local/bin").join(name),
            std::path::PathBuf::from("/opt/homebrew/bin").join(name),
            std::path::PathBuf::from("/usr/local/bin").join(name),
            home.join(".cargo/bin").join(name),
        ];

        for cand in &candidates {
            if cand.is_file() {
                return cand.clone();
            }
            #[cfg(windows)]
            {
                for ext in &["exe", "cmd", "bat"] {
                    let with_ext = cand.with_extension(ext);
                    if with_ext.is_file() {
                        return with_ext;
                    }
                }
            }
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            let p = std::path::Path::new(dir).join(name);
            if p.is_file() {
                return p;
            }
            #[cfg(windows)]
            {
                for ext in &["exe", "cmd", "bat"] {
                    let with_ext = p.with_extension(ext);
                    if with_ext.is_file() {
                        return with_ext;
                    }
                }
            }
        }
    }

    std::path::PathBuf::from(name)
}

/// Cross-platform helper to check if a binary exists in PATH or user local bin directory
pub fn is_binary_available(name: &str) -> bool {
    let resolved = resolve_binary_path(name);
    if resolved.is_absolute() {
        resolved.is_file()
    } else {
        std::process::Command::new(name)
            .arg("--version")
            .output()
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_disabled_providers() {
        let config = Config {
            enabled_providers: vec!["codex".into(), "antigravity".into()],
            ..Config::default()
        };
        let ids: Vec<_> = enabled_providers(&config).iter().map(|p| p.id()).collect();
        assert_eq!(ids, ["codex", "antigravity", "antigravity-argon"]);
    }

    #[test]
    fn empty_enabled_list_disables_every_provider() {
        let config = Config {
            enabled_providers: Vec::new(),
            ..Config::default()
        };
        assert!(enabled_providers(&config).is_empty());
    }

    #[test]
    fn antigravity_aliases_do_not_schedule_duplicate_pings() {
        let config = Config {
            enabled_providers: vec!["agy".into(), "antigravity".into(), "agy".into()],
            ..Config::default()
        };
        let ids: Vec<_> = enabled_providers(&config).iter().map(|p| p.id()).collect();
        assert_eq!(ids, ["antigravity", "antigravity-argon"]);
    }
}
