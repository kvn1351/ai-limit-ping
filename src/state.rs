use crate::reset_credits::ResetCreditReminders;
use crate::provider::{LimitWindow, UsageSnapshot};

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
    #[serde(default)]
    pub window: LimitWindow,
    /// Exact UTC timestamp when the primary quota window will reset
    pub window_resets_at: Option<DateTime<Utc>>,
    /// Exact UTC timestamp for secondary window (e.g. weekly cap)
    pub secondary_resets_at: Option<DateTime<Utc>>,
    /// Current primary window used percentage (0.0 - 100.0)
    pub used_percent: Option<f64>,
    /// Secondary window used percentage
    pub secondary_used_percent: Option<f64>,
    /// Whether the primary quota window is currently active
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
            window: LimitWindow::FiveHour,
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

    pub fn apply_usage(&mut self, usage: &UsageSnapshot, window: LimitWindow) {
        self.window = window;
        self.is_active = usage.is_active;
        self.window_resets_at = usage.resets_at;
        self.used_percent = usage.used_percent;
        self.secondary_resets_at = usage.secondary_resets_at;
        self.secondary_used_percent = usage.secondary_used_percent;
        self.last_checked_at = Some(Utc::now());
    }

    pub fn needs_usage_refresh(&self, expected_window: LimitWindow, force: bool) -> bool {
        self.needs_usage_refresh_at(expected_window, force, Utc::now())
    }

    fn needs_usage_refresh_at(&self, expected_window: LimitWindow, force: bool, now: DateTime<Utc>) -> bool {
        if force || self.window != expected_window || self.last_checked_at.is_none() {
            return true;
        }
        if self.window == LimitWindow::Weekly {
            if self.used_percent == Some(0.0) {
                return self.is_due_for_ping_at(0, now);
            }
            return self.window_resets_at.map(|reset| now >= reset).unwrap_or(true);
        }
        match self.window_resets_at {
            Some(reset) => (now - reset).num_seconds() > 60 || (!self.is_active && now >= reset),
            None => true,
        }
    }

    /// Returns how many seconds remain until reset (negative if already reset)
    pub fn remaining_seconds(&self) -> Option<i64> {
        self.window_resets_at.map(|reset| {
            let now = Utc::now();
            (reset - now).num_seconds()
        })
    }

    /// Whether this quota needs a ping; weekly quotas must be confirmed completely unused.
    pub fn is_due_for_ping(&self, buffer_seconds: u64) -> bool {
        self.is_due_for_ping_at(buffer_seconds, Utc::now())
    }

    fn is_due_for_ping_at(&self, buffer_seconds: u64, now: DateTime<Utc>) -> bool {
        if self.window == LimitWindow::Weekly {
            if self.used_percent != Some(0.0) {
                return false;
            }
            if self.last_ping_at.is_some_and(|last| (now - last).num_seconds() < 120) {
                return false;
            }
            return self.window_resets_at
                .map(|reset| now >= reset + chrono::Duration::seconds(buffer_seconds as i64))
                .unwrap_or(true);
        }
        if !self.is_active {
            // If window is free / inactive, it is due for ping!
            // Anti-rapid-loop guard: do not re-ping if we already pinged within the last 120 seconds
            if let Some(last_ping) = self.last_ping_at {
                if (now - last_ping).num_seconds() < 120 {
                    return false;
                }
            }
            return true;
        }

        match self.window_resets_at {
            Some(reset) => {
                let due_time = reset + chrono::Duration::seconds(buffer_seconds as i64);
                now >= due_time
            }
            None => {
                // If reset time is not available from API (e.g. Claude Code CLI fallback),
                // rely on last_ping_at to ensure we only ping once per 5 hours!
                match self.last_ping_at {
                    Some(last_ping) => {
                        let next_due = last_ping
                            + chrono::Duration::hours(5)
                            + chrono::Duration::seconds(buffer_seconds as i64);
                        now >= next_due
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
    #[serde(default)]
    pub reset_credit_reminders: ResetCreditReminders,
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

    #[test]
    fn weekly_quota_must_be_full_even_after_its_cached_deadline_passes() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut state = ProviderState::new("antigravity-argon", "Antigravity Argon");
        state.window = LimitWindow::Weekly;
        state.window_resets_at = Some(now - chrono::Duration::seconds(10));
        for used in [None, Some(0.000001), Some(100.0)] {
            state.used_percent = used;
            assert!(!state.is_due_for_ping_at(5, now));
        }
        state.used_percent = Some(0.0);
        assert!(state.is_due_for_ping_at(5, now));
    }

    #[test]
    fn consumed_weekly_quota_is_cached_until_reset_but_refreshed_on_wake() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut state = ProviderState::new("antigravity-argon", "Antigravity Argon");
        state.window = LimitWindow::Weekly;
        state.used_percent = Some(0.000001);
        state.last_checked_at = Some(now);
        state.window_resets_at = Some(now + chrono::Duration::days(7));
        assert!(!state.needs_usage_refresh_at(LimitWindow::Weekly, false, now));
        assert!(state.needs_usage_refresh_at(LimitWindow::Weekly, true, now));
        assert!(state.needs_usage_refresh_at(LimitWindow::Weekly, false, now + chrono::Duration::days(7)));
    }

    #[test]
    fn full_weekly_quota_respects_ping_cooldown_and_failure_backoff() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut state = ProviderState::new("antigravity-argon", "Antigravity Argon");
        state.window = LimitWindow::Weekly;
        state.used_percent = Some(0.0);
        state.last_checked_at = Some(now);
        state.last_ping_at = Some(now);
        assert!(!state.is_due_for_ping_at(5, now + chrono::Duration::seconds(119)));
        assert!(!state.needs_usage_refresh_at(LimitWindow::Weekly, false, now + chrono::Duration::seconds(119)));
        assert!(state.is_due_for_ping_at(5, now + chrono::Duration::seconds(120)));

        state.window_resets_at = Some(now + chrono::Duration::minutes(15));
        assert!(!state.is_due_for_ping_at(5, now + chrono::Duration::minutes(14)));
        assert!(!state.needs_usage_refresh_at(LimitWindow::Weekly, false, now + chrono::Duration::minutes(14)));
        assert!(state.is_due_for_ping_at(5, now + chrono::Duration::minutes(15) + chrono::Duration::seconds(5)));
    }
}
