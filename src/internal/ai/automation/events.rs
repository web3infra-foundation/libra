//! Automation event types for tracking rule trigger and execution results.
//!
//! 用于跟踪规则触发和执行结果的自动化事件类型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::internal::ai::{automation::config::AutomationTrigger, hooks::HookEvent};

pub const VCS_EVENT_POST_ADD: &str = "post_add";
pub const VCS_EVENT_POST_BRANCH: &str = "post_branch";
pub const VCS_EVENT_POST_COMMIT: &str = "post_commit";
pub const VCS_EVENT_POST_PUSH: &str = "post_push";
pub const VCS_EVENT_POST_SWITCH: &str = "post_switch";

#[derive(Debug, Error)]
pub enum AutomationError {
    #[error("failed to parse automation config: {0}")]
    ConfigParse(String),
    #[error("invalid automation config: {0}")]
    ConfigValidation(String),
    #[error("unsupported cron schedule `{0}`")]
    UnsupportedCron(String),
    #[error("automation database error: {0}")]
    Database(String),
    #[error("automation action failed: {0}")]
    Action(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunStatus {
    Succeeded,
    Failed,
    ApprovalRequired,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutomationRuntimeEvent {
    Hook { event: HookEvent },
    Vcs { event: String },
}

impl AutomationRuntimeEvent {
    pub fn hook(event: HookEvent) -> Self {
        Self::Hook { event }
    }

    pub fn vcs(event: impl Into<String>) -> Self {
        Self::Vcs {
            event: event.into(),
        }
    }

    pub fn matches_trigger(&self, trigger: &AutomationTrigger) -> bool {
        match (self, trigger) {
            (
                Self::Hook { event },
                AutomationTrigger::Hook {
                    event: trigger_event,
                },
            ) => event == trigger_event,
            (
                Self::Vcs { event },
                AutomationTrigger::Vcs {
                    event: trigger_event,
                },
            ) => event == trigger_event,
            _ => false,
        }
    }
}

impl AutomationRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::ApprovalRequired => "approval_required",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AutomationError> {
        match value {
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "approval_required" => Ok(Self::ApprovalRequired),
            "skipped" => Ok(Self::Skipped),
            other => Err(AutomationError::Database(format!(
                "unknown automation status `{other}`"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationRunResult {
    pub id: String,
    pub rule_id: String,
    pub trigger_kind: String,
    pub action_kind: String,
    pub status: AutomationRunStatus,
    pub message: String,
    pub details: Value,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

impl AutomationRunResult {
    pub fn new(
        rule_id: impl Into<String>,
        trigger_kind: impl Into<String>,
        action_kind: impl Into<String>,
        status: AutomationRunStatus,
        message: impl Into<String>,
        details: Value,
        started_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            rule_id: rule_id.into(),
            trigger_kind: trigger_kind.into(),
            action_kind: action_kind.into(),
            status,
            message: message.into(),
            details,
            started_at,
            finished_at: Utc::now(),
        }
    }

    pub fn skipped(rule_id: impl Into<String>, reason: impl Into<String>) -> Self {
        let now = Utc::now();
        Self::new(
            rule_id,
            "manual",
            "none",
            AutomationRunStatus::Skipped,
            reason,
            json!({}),
            now,
        )
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn automation_error_display_pins_each_variant() {
        assert_eq!(
            AutomationError::ConfigParse("invalid toml".to_string()).to_string(),
            "failed to parse automation config: invalid toml",
        );
        assert_eq!(
            AutomationError::ConfigValidation("missing action".to_string()).to_string(),
            "invalid automation config: missing action",
        );
        assert_eq!(
            AutomationError::UnsupportedCron("@hourly".to_string()).to_string(),
            "unsupported cron schedule `@hourly`",
        );
        assert_eq!(
            AutomationError::Database("connection lost".to_string()).to_string(),
            "automation database error: connection lost",
        );
        assert_eq!(
            AutomationError::Action("shell exit 1".to_string()).to_string(),
            "automation action failed: shell exit 1",
        );
    }

    #[test]
    fn vcs_event_constants_pin_wire_format_names() {
        // INVARIANT: VCS event constants are the literal strings the
        // command layer passes into `dispatch_vcs_event_to_history`
        // and that `has_matching_vcs_rule` compares byte-for-byte
        // (see runtime::tests::has_matching_vcs_rule_requires_exact_event_string).
        // A silent rename here would mute every rule keyed on the old name.
        assert_eq!(VCS_EVENT_POST_ADD, "post_add");
        assert_eq!(VCS_EVENT_POST_BRANCH, "post_branch");
        assert_eq!(VCS_EVENT_POST_COMMIT, "post_commit");
        assert_eq!(VCS_EVENT_POST_PUSH, "post_push");
        assert_eq!(VCS_EVENT_POST_SWITCH, "post_switch");
    }

    #[test]
    fn automation_run_status_as_str_pins_wire_format() {
        // INVARIANT: `as_str` and `parse` form a round-trip — the
        // persisted history column uses these literal strings. A
        // silent rename would orphan every existing row.
        assert_eq!(AutomationRunStatus::Succeeded.as_str(), "succeeded");
        assert_eq!(AutomationRunStatus::Failed.as_str(), "failed");
        assert_eq!(
            AutomationRunStatus::ApprovalRequired.as_str(),
            "approval_required"
        );
        assert_eq!(AutomationRunStatus::Skipped.as_str(), "skipped");
    }

    #[test]
    fn automation_run_status_parse_round_trips_every_variant() {
        for status in [
            AutomationRunStatus::Succeeded,
            AutomationRunStatus::Failed,
            AutomationRunStatus::ApprovalRequired,
            AutomationRunStatus::Skipped,
        ] {
            let parsed = AutomationRunStatus::parse(status.as_str())
                .expect("as_str must be accepted by parse");
            assert_eq!(parsed, status, "round-trip must preserve variant");
        }
    }

    #[test]
    fn automation_run_status_parse_rejects_unknown_string_as_database_error() {
        let err =
            AutomationRunStatus::parse("running").expect_err("unknown status must fail to parse");
        match err {
            AutomationError::Database(msg) => {
                assert!(
                    msg.contains("unknown automation status"),
                    "must explain the failure: {msg}"
                );
                assert!(msg.contains("running"), "must echo the bad token: {msg}");
            }
            other => panic!("expected Database, got {other:?}"),
        }
    }

    #[test]
    fn automation_run_status_parse_is_case_sensitive_and_strict() {
        // INVARIANT: stored values use lowercase snake_case. Accepting
        // mixed case would silently double the writeable surface and
        // let upstreams persist different bytes for the same logical
        // status.
        assert!(AutomationRunStatus::parse("Succeeded").is_err());
        assert!(AutomationRunStatus::parse("SUCCEEDED").is_err());
        assert!(AutomationRunStatus::parse("").is_err());
        assert!(AutomationRunStatus::parse("approval-required").is_err());
    }

    #[test]
    fn runtime_event_constructors_produce_corresponding_variants() {
        assert!(matches!(
            AutomationRuntimeEvent::hook(HookEvent::SessionEnd),
            AutomationRuntimeEvent::Hook { .. }
        ));
        assert!(matches!(
            AutomationRuntimeEvent::vcs("post_commit"),
            AutomationRuntimeEvent::Vcs { .. }
        ));
    }

    #[test]
    fn runtime_event_vcs_accepts_owned_and_borrowed_strings() {
        // `Into<String>` blanket: confirm the convenience constructor
        // works for both `&str` and `String` without manual cloning,
        // matching how dispatcher call sites supply the value.
        let from_str = AutomationRuntimeEvent::vcs("post_push");
        let from_owned = AutomationRuntimeEvent::vcs("post_push".to_string());
        match (from_str, from_owned) {
            (
                AutomationRuntimeEvent::Vcs { event: a },
                AutomationRuntimeEvent::Vcs { event: b },
            ) => {
                assert_eq!(a, "post_push");
                assert_eq!(b, "post_push");
            }
            other => panic!("expected both to be Vcs variants, got {other:?}"),
        }
    }

    #[test]
    fn matches_trigger_aligns_hook_to_hook_only() {
        let event = AutomationRuntimeEvent::hook(HookEvent::SessionEnd);
        assert!(event.matches_trigger(&AutomationTrigger::Hook {
            event: HookEvent::SessionEnd
        }));
        assert!(!event.matches_trigger(&AutomationTrigger::Hook {
            event: HookEvent::SessionStart
        }));
        // INVARIANT: cross-kind triggers never match — a hook event
        // must not satisfy a cron or vcs trigger and vice versa.
        assert!(!event.matches_trigger(&AutomationTrigger::Cron {
            schedule: "@hourly".to_string()
        }));
        assert!(!event.matches_trigger(&AutomationTrigger::Vcs {
            event: "session_end".to_string()
        }));
    }

    #[test]
    fn matches_trigger_aligns_vcs_to_vcs_only_with_exact_event_name() {
        let event = AutomationRuntimeEvent::vcs("post_commit");
        assert!(event.matches_trigger(&AutomationTrigger::Vcs {
            event: "post_commit".to_string()
        }));
        // Case-sensitive: must not match `Post_Commit`.
        assert!(!event.matches_trigger(&AutomationTrigger::Vcs {
            event: "Post_Commit".to_string()
        }));
        assert!(!event.matches_trigger(&AutomationTrigger::Hook {
            event: HookEvent::SessionEnd
        }));
        assert!(!event.matches_trigger(&AutomationTrigger::Cron {
            schedule: "@hourly".to_string()
        }));
    }

    #[test]
    fn automation_run_result_new_populates_uuid_and_timestamps() {
        let before = Utc::now();
        let started = Utc.with_ymd_and_hms(2026, 5, 20, 10, 0, 0).unwrap();
        let result = AutomationRunResult::new(
            "rule_x",
            "cron",
            "prompt",
            AutomationRunStatus::Succeeded,
            "ok",
            json!({"foo": 1}),
            started,
        );
        let after = Utc::now();

        assert_eq!(result.rule_id, "rule_x");
        assert_eq!(result.trigger_kind, "cron");
        assert_eq!(result.action_kind, "prompt");
        assert_eq!(result.status, AutomationRunStatus::Succeeded);
        assert_eq!(result.message, "ok");
        assert_eq!(result.details, json!({"foo": 1}));
        assert_eq!(result.started_at, started);
        // `id` must be a valid UUID v4 hex form; failing to parse
        // signals an upstream change in the id generator.
        Uuid::parse_str(&result.id).expect("id must parse as a UUID");
        // `finished_at` must be sampled from `Utc::now()` at construction.
        // The caller-supplied `started_at` is independent and may be in
        // the future relative to the real wall clock during testing, so
        // the assertion only constrains `finished_at` against the
        // surrounding `Utc::now()` window.
        assert!(
            result.finished_at >= before && result.finished_at <= after,
            "finished_at must land within [before, after]: {} not in [{before}, {after}]",
            result.finished_at
        );
    }

    #[test]
    fn automation_run_result_skipped_emits_manual_none_envelope() {
        // INVARIANT: the `skipped` shortcut shapes the row the
        // dispatcher writes when no rule actually ran. trigger_kind
        // = "manual" and action_kind = "none" form a stable filter
        // key used by automation analytics — renaming either string
        // would silently break dashboards.
        let result = AutomationRunResult::skipped("rule_y", "no enabled match");
        assert_eq!(result.rule_id, "rule_y");
        assert_eq!(result.trigger_kind, "manual");
        assert_eq!(result.action_kind, "none");
        assert_eq!(result.status, AutomationRunStatus::Skipped);
        assert_eq!(result.message, "no enabled match");
        assert_eq!(result.details, json!({}));
        Uuid::parse_str(&result.id).expect("id must parse as a UUID");
    }

    #[test]
    fn automation_run_result_new_generates_distinct_ids() {
        let started = Utc.with_ymd_and_hms(2026, 5, 20, 10, 0, 0).unwrap();
        let a = AutomationRunResult::new(
            "rule",
            "cron",
            "prompt",
            AutomationRunStatus::Succeeded,
            "",
            json!({}),
            started,
        );
        let b = AutomationRunResult::new(
            "rule",
            "cron",
            "prompt",
            AutomationRunStatus::Succeeded,
            "",
            json!({}),
            started,
        );
        assert_ne!(a.id, b.id, "every run must get a fresh UUID");
    }
}
