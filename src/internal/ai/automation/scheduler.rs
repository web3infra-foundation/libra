//! Automation scheduler for time-based rule execution.
//!
//! 基于时间的规则执行的自动化调度器。

use chrono::{DateTime, Timelike, Utc};

use crate::internal::ai::automation::{
    config::{AutomationConfig, AutomationRule, AutomationTrigger},
    events::{AutomationError, AutomationRunResult, AutomationRuntimeEvent},
    executor::AutomationExecutor,
};

pub struct AutomationScheduler {
    config: AutomationConfig,
}

impl AutomationScheduler {
    pub fn new(config: AutomationConfig) -> Self {
        Self { config }
    }

    pub fn due_rules_at(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<&AutomationRule>, AutomationError> {
        let mut due = Vec::new();
        for rule in &self.config.rules {
            if !rule.enabled {
                continue;
            }
            if let AutomationTrigger::Cron { schedule } = &rule.trigger
                && cron_due(schedule, now)?
            {
                due.push(rule);
            }
        }
        Ok(due)
    }

    pub async fn run_due_at(
        &self,
        now: DateTime<Utc>,
        executor: &AutomationExecutor,
    ) -> Result<Vec<AutomationRunResult>, AutomationError> {
        let due = self.due_rules_at(now)?;
        let mut results = Vec::with_capacity(due.len());
        for rule in due {
            results.push(executor.execute_rule(rule, rule.trigger.clone()).await);
        }
        Ok(results)
    }

    pub fn matching_event_rules(&self, event: &AutomationRuntimeEvent) -> Vec<&AutomationRule> {
        self.config
            .rules
            .iter()
            .filter(|rule| rule.enabled && event.matches_trigger(&rule.trigger))
            .collect()
    }

    pub async fn run_event(
        &self,
        event: AutomationRuntimeEvent,
        executor: &AutomationExecutor,
    ) -> Result<Vec<AutomationRunResult>, AutomationError> {
        let matching = self.matching_event_rules(&event);
        let mut results = Vec::with_capacity(matching.len());
        for rule in matching {
            results.push(executor.execute_rule(rule, rule.trigger.clone()).await);
        }
        Ok(results)
    }
}

fn cron_due(schedule: &str, now: DateTime<Utc>) -> Result<bool, AutomationError> {
    let schedule = schedule.trim();
    if schedule == "@hourly" {
        return Ok(now.minute() == 0 && now.second() == 0);
    }

    let parts = schedule.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 5 {
        return Err(AutomationError::UnsupportedCron(schedule.to_string()));
    }
    let minute = parts[0];
    if let Some(step) = minute.strip_prefix("*/") {
        let step = step
            .parse::<u32>()
            .map_err(|_| AutomationError::UnsupportedCron(schedule.to_string()))?;
        if step == 0 {
            return Err(AutomationError::UnsupportedCron(schedule.to_string()));
        }
        return Ok(now.minute().is_multiple_of(step) && now.second() == 0);
    }
    if let Ok(exact_minute) = minute.parse::<u32>() {
        return Ok(now.minute() == exact_minute && now.second() == 0);
    }

    Err(AutomationError::UnsupportedCron(schedule.to_string()))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::internal::ai::{
        automation::config::{AutomationAction, AutomationRule, AutomationTrigger},
        hooks::HookEvent,
    };

    fn ts(min: u32, sec: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 20, 10, min, sec).unwrap()
    }

    #[test]
    fn cron_due_at_hourly_only_at_minute_zero_second_zero() {
        // INVARIANT: `@hourly` means the top of the hour. A drift to
        // accept any second-of-minute would multiply trigger counts.
        assert!(cron_due("@hourly", ts(0, 0)).unwrap());
        assert!(!cron_due("@hourly", ts(0, 1)).unwrap());
        assert!(!cron_due("@hourly", ts(1, 0)).unwrap());
    }

    #[test]
    fn cron_due_strips_leading_trailing_whitespace_in_schedule() {
        assert!(cron_due("  @hourly  ", ts(0, 0)).unwrap());
    }

    #[test]
    fn cron_due_for_step_minutes_fires_only_at_second_zero() {
        assert!(cron_due("*/15 * * * *", ts(15, 0)).unwrap());
        assert!(cron_due("*/15 * * * *", ts(30, 0)).unwrap());
        assert!(cron_due("*/15 * * * *", ts(0, 0)).unwrap());
        assert!(!cron_due("*/15 * * * *", ts(15, 1)).unwrap());
        assert!(!cron_due("*/15 * * * *", ts(7, 0)).unwrap());
    }

    #[test]
    fn cron_due_for_exact_minute_fires_only_at_second_zero() {
        assert!(cron_due("42 * * * *", ts(42, 0)).unwrap());
        assert!(!cron_due("42 * * * *", ts(42, 30)).unwrap());
        assert!(!cron_due("42 * * * *", ts(41, 0)).unwrap());
    }

    #[test]
    fn cron_due_rejects_schedule_with_wrong_field_count() {
        // INVARIANT: the toy cron parser supports exactly the 5-field
        // crontab form (besides `@hourly`). Any drift would silently
        // alter scheduling semantics on upgrade.
        for bad in [
            "",
            "*",
            "* * * *",
            "* * * * * *",
            "minute hour dom mon dow extra",
        ] {
            let err = cron_due(bad, ts(0, 0)).expect_err("must reject malformed schedule");
            match err {
                AutomationError::UnsupportedCron(msg) => {
                    assert!(
                        msg.trim() == bad.trim(),
                        "error must echo the trimmed schedule, got {msg:?}"
                    );
                }
                other => panic!("expected UnsupportedCron, got {other:?}"),
            }
        }
    }

    #[test]
    fn cron_due_rejects_zero_step_minute() {
        let err = cron_due("*/0 * * * *", ts(0, 0)).expect_err("zero step must fail");
        assert!(matches!(err, AutomationError::UnsupportedCron(_)));
    }

    #[test]
    fn cron_due_rejects_non_numeric_step_minute() {
        let err = cron_due("*/x * * * *", ts(0, 0)).expect_err("non-numeric step must fail");
        assert!(matches!(err, AutomationError::UnsupportedCron(_)));
    }

    #[test]
    fn cron_due_rejects_unsupported_minute_field() {
        // INVARIANT: only `*/N` and bare integer minute syntax are
        // accepted. Lists, ranges, and `*` alone are not supported —
        // a future change should add new variants explicitly.
        for bad_minute in ["1,2,3", "0-5", "*"] {
            let schedule = format!("{bad_minute} * * * *");
            let err =
                cron_due(&schedule, ts(0, 0)).expect_err("unsupported minute syntax must fail");
            assert!(
                matches!(err, AutomationError::UnsupportedCron(_)),
                "schedule {schedule:?} must fail UnsupportedCron, got {err:?}"
            );
        }
    }

    fn cron_rule(id: &str, schedule: &str, enabled: bool) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            enabled,
            trigger: AutomationTrigger::Cron {
                schedule: schedule.to_string(),
            },
            action: AutomationAction::Prompt {
                prompt: "noop".to_string(),
            },
        }
    }

    fn hook_rule(id: &str, event: HookEvent, enabled: bool) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            enabled,
            trigger: AutomationTrigger::Hook { event },
            action: AutomationAction::Prompt {
                prompt: "noop".to_string(),
            },
        }
    }

    #[test]
    fn due_rules_at_skips_disabled_cron_rules() {
        let cfg = AutomationConfig {
            rules: vec![
                cron_rule("on", "@hourly", true),
                cron_rule("off", "@hourly", false),
            ],
        };
        let scheduler = AutomationScheduler::new(cfg);
        let due = scheduler.due_rules_at(ts(0, 0)).unwrap();
        assert_eq!(
            due.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["on"]
        );
    }

    #[test]
    fn due_rules_at_ignores_non_cron_triggers() {
        // INVARIANT: hook / vcs triggers must not appear in cron
        // scheduling output regardless of `now`; otherwise the cron
        // dispatch loop would fire event-only rules off the wall clock.
        let cfg = AutomationConfig {
            rules: vec![hook_rule("h", HookEvent::SessionEnd, true)],
        };
        let scheduler = AutomationScheduler::new(cfg);
        let due = scheduler.due_rules_at(ts(0, 0)).unwrap();
        assert!(due.is_empty());
    }

    #[test]
    fn due_rules_at_propagates_unsupported_cron_error() {
        let cfg = AutomationConfig {
            rules: vec![cron_rule("bad", "??? not a schedule ???", true)],
        };
        let scheduler = AutomationScheduler::new(cfg);
        let err = scheduler
            .due_rules_at(ts(0, 0))
            .expect_err("malformed cron must propagate");
        assert!(matches!(err, AutomationError::UnsupportedCron(_)));
    }

    #[test]
    fn matching_event_rules_filters_by_enabled_and_trigger() {
        let cfg = AutomationConfig {
            rules: vec![
                hook_rule("active_session_end", HookEvent::SessionEnd, true),
                hook_rule("disabled_session_end", HookEvent::SessionEnd, false),
                hook_rule("active_session_start", HookEvent::SessionStart, true),
            ],
        };
        let scheduler = AutomationScheduler::new(cfg);
        let event = AutomationRuntimeEvent::hook(HookEvent::SessionEnd);
        let matching = scheduler.matching_event_rules(&event);
        assert_eq!(
            matching.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["active_session_end"]
        );
    }

    #[test]
    fn matching_event_rules_returns_empty_when_no_trigger_matches() {
        let cfg = AutomationConfig {
            rules: vec![cron_rule("any_cron", "@hourly", true)],
        };
        let scheduler = AutomationScheduler::new(cfg);
        let event = AutomationRuntimeEvent::vcs("post_commit");
        assert!(scheduler.matching_event_rules(&event).is_empty());
    }
}
