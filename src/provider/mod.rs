pub mod antigravity;
pub mod claude;
pub mod codex;

use anyhow::Result;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct UsageSnapshot {
    /// Whether the 5-hour rolling window is active and counting down
    pub is_active: bool,
    /// The exact UTC time when the 5-hour window will reset
    pub resets_at: Option<DateTime<Utc>>,
    /// Percentage used in the 5-hour window (0.0 to 100.0)
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

    /// Checks if CLI tool or credentials for this provider are installed/configured
    async fn is_available(&self) -> bool;

    /// Fetches the latest usage limits and reset timestamps from the API / CLI
    async fn fetch_usage(&self) -> Result<UsageSnapshot>;

    /// Triggers a synthetic minimal turn to restart the 5-hour rolling window
    async fn trigger_ping(&self, dry_run: bool) -> Result<PingResult>;

    /// Detects if an active user turn is already in progress, avoiding synthetic interference
    async fn is_active_task_running(&self) -> Result<bool>;
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
