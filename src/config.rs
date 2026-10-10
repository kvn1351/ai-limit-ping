use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Number of seconds to wait past the exact reset time before pinging
    pub reset_buffer_seconds: u64,

    /// Coarse check interval in seconds to catch sleep/wake jumps
    pub coarse_tick_seconds: u64,

    /// Which providers are enabled: "codex", "antigravity", "claude"
    pub enabled_providers: Vec<String>,

    /// Override model for Codex (e.g. "gpt-5.6-luna", or None for cheapest/default)
    pub codex_model: Option<String>,

    /// Override model for Antigravity (e.g. "gemini-3.8-flash-low", or None for default)
    pub antigravity_model: Option<String>,

    /// Override model for Claude Code (or None for default)
    pub claude_model: Option<String>,

    /// Whether to send OS desktop notifications on pings
    pub notify: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            reset_buffer_seconds: 5,
            coarse_tick_seconds: 30,
            enabled_providers: vec![
                "codex".to_string(),
                "antigravity".to_string(),
                "claude".to_string(),
            ],
            codex_model: Some("gpt-5.6-luna".to_string()),
            antigravity_model: Some("gemini-3.8-flash-low".to_string()),
            claude_model: None,
            notify: true,
        }
    }
}

impl Config {
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ai-limit-ping")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    pub fn load_or_default() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(cfg) = serde_json::from_str(&content) {
                    return cfg;
                }
            }
        }
        let cfg = Self::default();
        let _ = cfg.save();
        cfg
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(Self::config_path(), content)?;
        Ok(())
    }
}
