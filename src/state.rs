use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderState {
    pub id: String,
    pub display_name: String,
    /// Exact UTC timestamp when the current 5-hour window will reset
    pub window_resets_at: Option<DateTime<Utc>>,
    /// Exact UTC timestamp for secondary window (e.g. weekly cap)
    pub secondary_resets_at: Option<DateTime<Utc>>,
    /// Current 5h window used percentage (0.0 - 100.0)
    pub used_percent: Option<f64>,
    /// Secondary window used percentage
    pub secondary_used_percent: Option<f64>,
    /// Whether a 5h window is currently active and counting down
    pub is_active: bool,
    /// When this provider's limit was last queried
    pub last_checked_at: Option<DateTime<Utc>>,
    /// When a ping was last fired
    pub last_ping_at: Option<DateTime<Utc>>,
    /// Description of the last ping result
    pub last_ping_status: Option<String>,
}

impl ProviderState {
    pub fn new(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            window_resets_at: None,
            secondary_resets_at: None,
            used_percent: None,
            secondary_used_percent: None,
            is_active: false,
            last_checked_at: None,
            last_ping_at: None,
            last_ping_status: None,
        }
    }

    /// Returns how many seconds remain until reset (negative if already reset)
    pub fn remaining_seconds(&self) -> Option<i64> {
        self.window_resets_at.map(|reset| {
            let now = Utc::now();
            (reset - now).num_seconds()
        })
    }

    /// Whether the 5h window is due for a ping (i.e. reset timestamp is in the past or window is free)
    pub fn is_due_for_ping(&self, buffer_seconds: u64) -> bool {
        if !self.is_active {
            // If window is free / inactive, it is due for ping!
            // Anti-rapid-loop guard: do not re-ping if we already pinged within the last 120 seconds
            if let Some(last_ping) = self.last_ping_at {
                if (Utc::now() - last_ping).num_seconds() < 120 {
                    return false;
                }
            }
            return true;
        }

        match self.window_resets_at {
            Some(reset) => {
                let due_time = reset + chrono::Duration::seconds(buffer_seconds as i64);
                Utc::now() >= due_time
            }
            None => {
                // If reset time is not available from API (e.g. Claude Code CLI fallback),
                // rely on last_ping_at to ensure we only ping once per 5 hours!
                match self.last_ping_at {
                    Some(last_ping) => {
                        let next_due = last_ping
                            + chrono::Duration::hours(5)
                            + chrono::Duration::seconds(buffer_seconds as i64);
                        Utc::now() >= next_due
                    }
                    None => true,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppState {
    pub providers: BTreeMap<String, ProviderState>,
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_update_check: Option<DateTime<Utc>>,
    #[serde(default)]
    pub latest_known_version: Option<String>,
}

impl AppState {
    pub fn state_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ai-limit-ping")
    }

    pub fn state_path() -> PathBuf {
        Self::state_dir().join("state.json")
    }

    pub fn load() -> Self {
        let path = Self::state_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(state) = serde_json::from_str(&content) {
                    return state;
                }
            }
        }
        Self::default()
    }

    pub fn save(&mut self) -> Result<()> {
        self.updated_at = Some(Utc::now());
        let dir = Self::state_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        let temp_path = dir.join(format!("state.json.tmp.{}", std::process::id()));
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&temp_path, content)?;
        fs::rename(temp_path, Self::state_path())?;
        Ok(())
    }

    pub fn get_provider_cloned(&self, id: &str, display_name: &str) -> ProviderState {
        self.providers
            .get(id)
            .cloned()
            .unwrap_or_else(|| ProviderState::new(id, display_name))
    }

    pub fn set_provider(&mut self, state: ProviderState) {
        self.providers.insert(state.id.clone(), state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_due_for_ping_when_inactive() {
        let mut p = ProviderState::new("codex", "Codex");
        p.is_active = false;
        p.last_ping_at = None;
        assert!(p.is_due_for_ping(5));

        // Anti-rapid-loop: if pinged 30 seconds ago, should NOT be due
        p.last_ping_at = Some(Utc::now() - chrono::Duration::seconds(30));
        assert!(!p.is_due_for_ping(5));

        // If pinged 150 seconds ago, should be due again
        p.last_ping_at = Some(Utc::now() - chrono::Duration::seconds(150));
        assert!(p.is_due_for_ping(5));
    }

    #[test]
    fn test_is_due_for_ping_when_active() {
        let mut p = ProviderState::new("codex", "Codex");
        p.is_active = true;
        // Window resets in 2 hours
        p.window_resets_at = Some(Utc::now() + chrono::Duration::hours(2));
        assert!(!p.is_due_for_ping(5));

        // Window reset 10 seconds ago
        p.window_resets_at = Some(Utc::now() - chrono::Duration::seconds(10));
        assert!(p.is_due_for_ping(5));
    }
}
