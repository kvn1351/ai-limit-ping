use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetCredit {
    pub id: String,
    pub title: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct ResetCreditInventory {
    pub account_id: String,
    pub credits: Vec<ResetCredit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetCreditReminder {
    pub credit_id: String,
    pub threshold_hours: u32,
    pub message: String,
}

/// The reminder state is kept separately for each account.  A threshold is a
/// high-water mark: smaller values are more urgent, so a credit can progress
/// from 24h to 3h to 1h without replaying a tier after a restart or a clock
/// correction.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResetCreditReminders {
    #[serde(default)]
    active_account_id: Option<String>,
    #[serde(default)]
    active_credits: BTreeMap<String, ResetCredit>,
    #[serde(default)]
    notified_thresholds: BTreeMap<String, BTreeMap<String, u32>>,
}

impl ResetCreditReminders {
    /// Replace the provider's current inventory and make it the inventory used
    /// by subsequent due checks.  History for other accounts is retained, but
    /// credits no longer present in this account are forgotten so a newly
    /// issued credit reusing an id can be notified normally.
    pub fn replace_inventory(&mut self, inventory: ResetCreditInventory) {
        let account_id = inventory.account_id;
        let mut credits = BTreeMap::new();
        for credit in inventory.credits {
            credits.insert(credit.id.clone(), credit);
        }

        let account_history = self
            .notified_thresholds
            .entry(account_id.clone())
            .or_default();
        account_history.retain(|credit_id, threshold| {
            credits.contains_key(credit_id) && is_threshold(*threshold)
        });

        self.active_account_id = Some(account_id);
        self.active_credits = credits;
    }

    /// Return at most one reminder per available, unexpired credit.  When a
    /// provider was offline long enough to cross several tiers, only the most
    /// urgent tier visible now is returned.
    pub fn due_reminders(&self, now: DateTime<Utc>) -> Vec<ResetCreditReminder> {
        self.due_credits(now)
            .map(|(credit, threshold_hours)| ResetCreditReminder {
                credit_id: credit.id.clone(),
                threshold_hours,
                message: format!(
                    "Reset credit \"{}\" expires in {}.",
                    credit.title,
                    format_remaining(credit.expires_at.unwrap() - now)
                ),
            })
            .collect()
    }

    pub fn has_due_reminders(&self, now: DateTime<Utc>) -> bool {
        self.due_credits(now).next().is_some()
    }

    fn due_credits(&self, now: DateTime<Utc>) -> impl Iterator<Item = (&ResetCredit, u32)> {
        let account_history = self
            .active_account_id
            .as_ref()
            .and_then(|account| self.notified_thresholds.get(account));
        self.active_credits.values().filter_map(move |credit| {
            let threshold_hours = threshold_for(credit.expires_at? - now)?;
            let already_notified = account_history
                .and_then(|history| history.get(&credit.id))
                .copied()
                .filter(|threshold| is_threshold(*threshold));
            if already_notified.is_some_and(|threshold| threshold_hours >= threshold) {
                return None;
            }
            Some((credit, threshold_hours))
        })
    }

    /// Record delivery of a reminder.  The smallest threshold ever delivered
    /// wins, making repeated calls or out-of-order delivery harmless.
    pub fn mark_notified(&mut self, reminder: &ResetCreditReminder) {
        if !is_threshold(reminder.threshold_hours) {
            return;
        }
        let Some(account_id) = self.active_account_id.as_deref() else {
            return;
        };

        let account_history = self
            .notified_thresholds
            .entry(account_id.to_owned())
            .or_default();
        account_history
            .entry(reminder.credit_id.clone())
            .and_modify(|threshold| *threshold = (*threshold).min(reminder.threshold_hours))
            .or_insert(reminder.threshold_hours);
    }
}

fn is_threshold(threshold_hours: u32) -> bool {
    matches!(threshold_hours, 1 | 3 | 24)
}

fn threshold_for(remaining: Duration) -> Option<u32> {
    if remaining <= Duration::zero() {
        return None;
    }
    if remaining <= Duration::hours(1) {
        Some(1)
    } else if remaining <= Duration::hours(3) {
        Some(3)
    } else if remaining <= Duration::hours(24) {
        Some(24)
    } else {
        None
    }
}

fn format_remaining(remaining: Duration) -> String {
    let seconds = remaining.num_seconds();
    if seconds == 0 {
        return "less than a second".to_owned();
    }
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;

    if hours > 0 {
        let mut result = format!("{} {}", hours, unit(hours, "hour"));
        if minutes > 0 {
            result.push_str(&format!(" {} {}", minutes, unit(minutes, "minute")));
        }
        return result;
    }
    if minutes > 0 {
        let mut result = format!("{} {}", minutes, unit(minutes, "minute"));
        if seconds > 0 {
            result.push_str(&format!(" {} {}", seconds, unit(seconds, "second")));
        }
        return result;
    }
    format!("{} {}", seconds, unit(seconds, "second"))
}

fn unit(value: i64, singular: &str) -> &'static str {
    if value == 1 {
        match singular {
            "hour" => "hour",
            "minute" => "minute",
            _ => "second",
        }
    } else {
        match singular {
            "hour" => "hours",
            "minute" => "minutes",
            _ => "seconds",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0).single().unwrap()
    }

    fn credit(id: &str, hours: i64) -> ResetCredit {
        ResetCredit {
            id: id.to_owned(),
            title: format!("Credit {id}"),
            expires_at: Some(now() + Duration::hours(hours)),
        }
    }

    fn inventory(account_id: &str, credits: Vec<ResetCredit>) -> ResetCreditInventory {
        ResetCreditInventory {
            account_id: account_id.to_owned(),
            credits,
        }
    }

    #[test]
    fn thresholds_are_inclusive_and_progress_in_order() {
        let mut state = ResetCreditReminders::default();
        state.replace_inventory(inventory("a", vec![credit("c", 24)]));
        assert!(state.due_reminders(now() - Duration::seconds(1)).is_empty());

        let reminders = state.due_reminders(now());
        assert_eq!(reminders[0].threshold_hours, 24);
        state.mark_notified(&reminders[0]);
        assert!(state.due_reminders(now() + Duration::hours(21) - Duration::seconds(1)).is_empty());

        let three_hours = now() + Duration::hours(21);
        let reminders = state.due_reminders(three_hours);
        assert_eq!(reminders[0].threshold_hours, 3);
        state.mark_notified(&reminders[0]);
        assert!(state.due_reminders(now() + Duration::hours(23) - Duration::seconds(1)).is_empty());

        let one_hour = now() + Duration::hours(23);
        let reminders = state.due_reminders(one_hour);
        assert_eq!(reminders[0].threshold_hours, 1);
        state.mark_notified(&reminders[0]);
        assert!(state.due_reminders(one_hour).is_empty());
    }

    #[test]
    fn cold_start_at_one_hour_skips_missed_tiers() {
        let mut state = ResetCreditReminders::default();
        state.replace_inventory(inventory("a", vec![credit("c", 1)]));

        let reminders = state.due_reminders(now());
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].threshold_hours, 1);
    }

    #[test]
    fn offline_gap_emits_only_current_most_urgent_tier() {
        let mut state = ResetCreditReminders::default();
        state.replace_inventory(inventory("a", vec![credit("c", 24)]));
        let initial = state.due_reminders(now()).pop().unwrap();
        state.mark_notified(&initial);

        let reminders = state.due_reminders(now() + Duration::hours(23));
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].threshold_hours, 1);
    }

    #[test]
    fn below_hour_message_uses_actual_duration() {
        let mut state = ResetCreditReminders::default();
        let mut c = credit("c", 1);
        c.expires_at = Some(now() + Duration::minutes(5));
        state.replace_inventory(inventory("a", vec![c]));

        let reminder = &state.due_reminders(now())[0];
        assert_eq!(reminder.threshold_hours, 1);
        assert!(reminder.message.contains("5 minutes"));
        assert!(!reminder.message.contains("1 hour"));
    }

    #[test]
    fn serde_restart_preserves_deduplication() {
        let mut state = ResetCreditReminders::default();
        state.replace_inventory(inventory("a", vec![credit("c", 2)]));
        let reminder = state.due_reminders(now())[0].clone();
        state.mark_notified(&reminder);

        let encoded = serde_json::to_string(&state).unwrap();
        let mut restarted: ResetCreditReminders = serde_json::from_str(&encoded).unwrap();
        restarted.replace_inventory(inventory("a", vec![credit("c", 2)]));
        assert!(restarted.due_reminders(now()).is_empty());
    }

    #[test]
    fn expired_nonexpiring_and_removed_credits_are_ignored() {
        let mut state = ResetCreditReminders::default();
        let mut expired = credit("expired", 1);
        expired.expires_at = Some(now());
        state.replace_inventory(inventory(
            "a",
            vec![
                expired,
                ResetCredit {
                    id: "never".to_owned(),
                    title: "Never".to_owned(),
                    expires_at: None,
                },
                credit("removed", 1),
            ],
        ));
        let reminder = state.due_reminders(now())[0].clone();
        assert_eq!(reminder.credit_id, "removed");
        state.mark_notified(&reminder);

        state.replace_inventory(inventory("a", vec![]));
        assert!(state.due_reminders(now()).is_empty());
        state.replace_inventory(inventory("a", vec![credit("removed", 1)]));
        assert_eq!(state.due_reminders(now()).len(), 1);
    }

    #[test]
    fn account_histories_do_not_cross_contaminate() {
        let mut state = ResetCreditReminders::default();
        state.replace_inventory(inventory("a", vec![credit("same", 2)]));
        let reminder = state.due_reminders(now())[0].clone();
        state.mark_notified(&reminder);

        state.replace_inventory(inventory("b", vec![credit("same", 2)]));
        assert_eq!(state.due_reminders(now()).len(), 1);

        state.replace_inventory(inventory("a", vec![credit("same", 2)]));
        assert!(state.due_reminders(now()).is_empty());
    }

    #[test]
    fn expiry_corrections_do_not_replay_covered_tier() {
        let mut state = ResetCreditReminders::default();
        state.replace_inventory(inventory("a", vec![credit("c", 2)]));
        let reminder = state.due_reminders(now())[0].clone();
        state.mark_notified(&reminder);

        state.replace_inventory(inventory("a", vec![credit("c", 10)]));
        assert!(state.due_reminders(now()).is_empty());
        state.replace_inventory(inventory("a", vec![credit("c", 2)]));
        assert!(state.due_reminders(now()).is_empty());
    }
}
