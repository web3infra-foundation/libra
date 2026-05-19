//! Protocol handling for Agent Codex WebSocket communication.
//!
//! The Codex executor (the external `codex` binary that backs Libra's agent runs)
//! speaks an app-server JSON-RPC schema over WebSocket. This module defines the small
//! enum we use to classify the hundreds of possible method names into the dozen-ish
//! "buckets" the rest of the runtime cares about.
//!
//! Method strings come from the `method` field of incoming notifications. We treat
//! anything we do not recognize as `Unknown` and let the caller decide whether to log
//! or drop it — schema additions in newer Codex builds therefore degrade gracefully.

/// Known notification methods we care about (app-server schema v2 subset).
///
/// Each variant maps to one or more raw method strings. Where the schema underwent
/// renaming (e.g. `turnStarted` → `turn/started`), both spellings are accepted so the
/// client works against current and legacy server builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    ThreadStarted,
    ThreadStatusChanged,
    ThreadNameUpdated,
    ThreadArchived,
    ThreadCompacted,
    ThreadClosed,
    TurnStarted,
    TurnCompleted,
    TokenUsageUpdated,
    PlanUpdated,
    PlanDelta,
    AgentMessageDelta,
    CommandExecutionOutputDelta,
    FileChangeOutputDelta,
    TaskStarted,
    TaskCompleted,
    ItemStarted,
    ItemCompleted,
    RequestApproval,
    RequestApprovalCommandExecution,
    RequestApprovalFileChange,
    RequestApprovalApplyPatch,
    RequestApprovalExec,
    Initialized,
    Unknown,
}

impl MethodKind {
    /// Classify a raw `method` string from a JSON-RPC notification.
    ///
    /// Functional scope: pattern-matches both legacy and current spellings. The catch-
    /// all `Unknown` arm is what makes the matching forward-compatible — new methods
    /// appear as `Unknown` until they are explicitly handled.
    ///
    /// Boundary conditions:
    /// - The order of the `m if ...` guard arms matters because more-specific prefixes
    ///   (e.g. `item/commandExecution/requestApproval`) are tested before the generic
    ///   `requestApproval` suffix arm.
    pub fn from(method: &str) -> Self {
        match method {
            "thread/started" => Self::ThreadStarted,
            "thread/status/changed" => Self::ThreadStatusChanged,
            "thread/name/updated" => Self::ThreadNameUpdated,
            "thread/archived" => Self::ThreadArchived,
            "thread/compacted" => Self::ThreadCompacted,
            "thread/closed" => Self::ThreadClosed,
            "turn/started" | "turnStarted" => Self::TurnStarted,
            "turn/completed" | "turnCompleted" => Self::TurnCompleted,
            "thread/tokenUsage/updated" | "tokenUsage" => Self::TokenUsageUpdated,
            "turn/plan/updated" | "plan/updated" => Self::PlanUpdated,
            "item/plan/delta" => Self::PlanDelta,
            "item/agentMessage/delta" => Self::AgentMessageDelta,
            "item/commandExecution/outputDelta" => Self::CommandExecutionOutputDelta,
            "item/fileChange/outputDelta" => Self::FileChangeOutputDelta,
            "codex/event/task_started" => Self::TaskStarted,
            "codex/event/task_complete" => Self::TaskCompleted,
            m if m.starts_with("item/") && m.ends_with("/started") => Self::ItemStarted,
            m if m.starts_with("item/") && m.ends_with("/completed") => Self::ItemCompleted,
            m if m.starts_with("item/commandExecution/requestApproval") => {
                Self::RequestApprovalCommandExecution
            }
            m if m.starts_with("item/fileChange/requestApproval") => {
                Self::RequestApprovalFileChange
            }
            m if m.starts_with("apply_patch_approval_request") => Self::RequestApprovalApplyPatch,
            m if m.starts_with("exec_approval_request") => Self::RequestApprovalExec,
            m if m.ends_with("requestApproval") || m.ends_with("/requestApproval") => {
                Self::RequestApproval
            }
            "initialized" => Self::Initialized,
            _ => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_lifecycle_methods_match_canonical_strings() {
        assert_eq!(
            MethodKind::from("thread/started"),
            MethodKind::ThreadStarted
        );
        assert_eq!(
            MethodKind::from("thread/status/changed"),
            MethodKind::ThreadStatusChanged
        );
        assert_eq!(
            MethodKind::from("thread/name/updated"),
            MethodKind::ThreadNameUpdated
        );
        assert_eq!(
            MethodKind::from("thread/archived"),
            MethodKind::ThreadArchived
        );
        assert_eq!(
            MethodKind::from("thread/compacted"),
            MethodKind::ThreadCompacted
        );
        assert_eq!(MethodKind::from("thread/closed"), MethodKind::ThreadClosed);
    }

    #[test]
    fn turn_methods_accept_both_legacy_and_current_spellings() {
        // INVARIANT: the schema rename `turnStarted → turn/started`
        // must remain backward-compatible. A regression that dropped
        // either spelling would silently turn one half of the
        // notifications into `Unknown`.
        assert_eq!(MethodKind::from("turn/started"), MethodKind::TurnStarted);
        assert_eq!(MethodKind::from("turnStarted"), MethodKind::TurnStarted);
        assert_eq!(
            MethodKind::from("turn/completed"),
            MethodKind::TurnCompleted
        );
        assert_eq!(MethodKind::from("turnCompleted"), MethodKind::TurnCompleted);
    }

    #[test]
    fn token_usage_methods_accept_both_spellings() {
        assert_eq!(
            MethodKind::from("thread/tokenUsage/updated"),
            MethodKind::TokenUsageUpdated
        );
        assert_eq!(
            MethodKind::from("tokenUsage"),
            MethodKind::TokenUsageUpdated
        );
    }

    #[test]
    fn plan_methods_accept_both_spellings() {
        assert_eq!(
            MethodKind::from("turn/plan/updated"),
            MethodKind::PlanUpdated
        );
        assert_eq!(MethodKind::from("plan/updated"), MethodKind::PlanUpdated);
        assert_eq!(MethodKind::from("item/plan/delta"), MethodKind::PlanDelta);
    }

    #[test]
    fn item_output_delta_methods_match_canonical_strings() {
        assert_eq!(
            MethodKind::from("item/agentMessage/delta"),
            MethodKind::AgentMessageDelta
        );
        assert_eq!(
            MethodKind::from("item/commandExecution/outputDelta"),
            MethodKind::CommandExecutionOutputDelta
        );
        assert_eq!(
            MethodKind::from("item/fileChange/outputDelta"),
            MethodKind::FileChangeOutputDelta
        );
    }

    #[test]
    fn task_lifecycle_methods_match_canonical_strings() {
        assert_eq!(
            MethodKind::from("codex/event/task_started"),
            MethodKind::TaskStarted
        );
        assert_eq!(
            MethodKind::from("codex/event/task_complete"),
            MethodKind::TaskCompleted
        );
    }

    #[test]
    fn item_started_and_completed_match_any_item_subpath() {
        // INVARIANT: `item/<any>/started` and `item/<any>/completed`
        // are bucketed into `ItemStarted` / `ItemCompleted`. A
        // regression that switched to an exact-match list would
        // silently mute every newly-added item kind.
        assert_eq!(
            MethodKind::from("item/agentMessage/started"),
            MethodKind::ItemStarted
        );
        assert_eq!(
            MethodKind::from("item/fileChange/started"),
            MethodKind::ItemStarted
        );
        assert_eq!(
            MethodKind::from("item/newKind123/started"),
            MethodKind::ItemStarted
        );
        assert_eq!(
            MethodKind::from("item/commandExecution/completed"),
            MethodKind::ItemCompleted
        );
        assert_eq!(
            MethodKind::from("item/anything/completed"),
            MethodKind::ItemCompleted
        );
    }

    #[test]
    fn item_started_does_not_match_unrelated_methods() {
        assert_eq!(MethodKind::from("started/item"), MethodKind::Unknown);
        assert_eq!(
            MethodKind::from("notitem/something/started"),
            MethodKind::Unknown
        );
    }

    #[test]
    fn request_approval_buckets_specific_prefixes_first() {
        // INVARIANT: the more-specific prefix arms (`item/commandExecution/
        // requestApproval`, `item/fileChange/requestApproval`) must run
        // BEFORE the generic `*requestApproval` arm. A regression that
        // re-ordered guards would collapse all specific approvals into
        // the generic bucket.
        assert_eq!(
            MethodKind::from("item/commandExecution/requestApproval"),
            MethodKind::RequestApprovalCommandExecution
        );
        assert_eq!(
            MethodKind::from("item/commandExecution/requestApproval/v2"),
            MethodKind::RequestApprovalCommandExecution
        );
        assert_eq!(
            MethodKind::from("item/fileChange/requestApproval"),
            MethodKind::RequestApprovalFileChange
        );
        assert_eq!(
            MethodKind::from("item/fileChange/requestApproval/v2"),
            MethodKind::RequestApprovalFileChange
        );
    }

    #[test]
    fn request_approval_apply_patch_and_exec_match_any_suffix() {
        assert_eq!(
            MethodKind::from("apply_patch_approval_request"),
            MethodKind::RequestApprovalApplyPatch
        );
        assert_eq!(
            MethodKind::from("apply_patch_approval_request_v2"),
            MethodKind::RequestApprovalApplyPatch
        );
        assert_eq!(
            MethodKind::from("exec_approval_request"),
            MethodKind::RequestApprovalExec
        );
        assert_eq!(
            MethodKind::from("exec_approval_request_v2"),
            MethodKind::RequestApprovalExec
        );
    }

    #[test]
    fn generic_request_approval_arm_catches_both_camel_case_and_path() {
        // INVARIANT: the trailing `requestApproval` (literal lowercase
        // suffix) and `/requestApproval` (path-style) forms must both
        // fall through to the generic bucket — agent servers emit
        // either depending on schema version. The match is
        // case-sensitive: a leading capital `R` in `Approval` would
        // not match (a regression test for this lives in
        // `unknown_is_the_default_for_unhandled_methods`).
        assert_eq!(
            MethodKind::from("someUnknownItem/requestApproval"),
            MethodKind::RequestApproval
        );
        // A plain camelCase form without a `/` separator must still
        // match the bare `ends_with("requestApproval")` arm as long
        // as the suffix is exactly `requestApproval` (lowercase r).
        assert_eq!(
            MethodKind::from("legacyrequestApproval"),
            MethodKind::RequestApproval
        );
    }

    #[test]
    fn initialized_is_recognised() {
        assert_eq!(MethodKind::from("initialized"), MethodKind::Initialized);
    }

    #[test]
    fn unknown_is_the_default_for_unhandled_methods() {
        // INVARIANT: forward-compatibility relies on `Unknown` being
        // the safe default. Without this, any new method emitted by
        // a newer Codex build would crash the client.
        assert_eq!(MethodKind::from(""), MethodKind::Unknown);
        assert_eq!(MethodKind::from("random/method"), MethodKind::Unknown);
        assert_eq!(MethodKind::from("turn/random"), MethodKind::Unknown);
        // Case-sensitive: "Initialized" must not collide with
        // "initialized".
        assert_eq!(MethodKind::from("Initialized"), MethodKind::Unknown);
        // Case-sensitive on the requestApproval suffix too:
        // `RequestApproval` (capital R) must NOT match the generic
        // bucket because the suffix arm uses lowercase `r`.
        assert_eq!(
            MethodKind::from("legacyRequestApproval"),
            MethodKind::Unknown
        );
    }
}
