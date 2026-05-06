use chrono::{DateTime, Timelike, Utc};

use crate::internal::ai::automation::{
    config::{AutomationConfig, AutomationRule, AutomationTrigger},
    events::{AutomationError, AutomationRunResult},
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
