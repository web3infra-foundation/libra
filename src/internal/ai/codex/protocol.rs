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
