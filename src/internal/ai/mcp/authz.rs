//! MCP authorization shim — Phase 5 schema-only placeholder.
//!
//! This module declares the [`McpAuthorizer`] trait alongside its operation,
//! decision and error types. The trait is **not yet wired** into the MCP
//! server request path (`server.rs`); it sits here so that Phase 5 hardening
//! work can later attach authorization checks to MCP `tools/call` and
//! `resources/read` paths without having to introduce the trait at the same
//! time.
//!
//! The shape of the trait is intentionally minimal and dyn-compatible so it
//! can be dropped into [`crate::internal::ai::mcp::server`] behind a single
//! `Arc<dyn McpAuthorizer>` field once Phase 5 wiring lands.

use async_trait::async_trait;

use crate::internal::ai::runtime::hardening::PrincipalContext;

/// A single MCP request shape that authorization decisions are made against.
///
/// Variants are borrow-only so callers don't need to clone request payloads
/// on every authorization check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpOperation<'a> {
    /// `tools/list` request.
    ListTools,
    /// `resources/list` request.
    ListResources,
    /// `resources/templates/list` request.
    ListResourceTemplates,
    /// `resources/read` for a specific URI.
    ReadResource { uri: &'a str },
    /// `tools/call` for a named tool.
    CallTool { tool_name: &'a str },
}

impl McpOperation<'_> {
    /// Human-readable label, used by audit and diagnostics surfaces.
    pub fn label(&self) -> &'static str {
        match self {
            Self::ListTools => "tools/list",
            Self::ListResources => "resources/list",
            Self::ListResourceTemplates => "resources/templates/list",
            Self::ReadResource { .. } => "resources/read",
            Self::CallTool { .. } => "tools/call",
        }
    }
}

/// Outcome of an authorization check.
///
/// `NeedsHuman` is used by Phase 5 to route the request through the existing
/// approval channel rather than auto-allowing or auto-denying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthzDecision {
    /// Operation is permitted; the server may proceed with the request.
    Allow,
    /// Operation is denied; the server returns an error to the client.
    Deny { reason: String },
    /// Operation requires human approval before the server proceeds.
    NeedsHuman { reason: String },
}

impl AuthzDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Allow => None,
            Self::Deny { reason } | Self::NeedsHuman { reason } => Some(reason.as_str()),
        }
    }
}

/// Failure modes for authorization checks themselves (distinct from a clean
/// `Deny` decision).
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthzError {
    /// Authorization backend could not produce a decision (e.g. policy file
    /// missing, cache unavailable).
    #[error("authorization backend failure: {0}")]
    Backend(String),
    /// Caller invoked the trait without a valid `PrincipalContext`.
    #[error("principal context unavailable: {0}")]
    PrincipalUnavailable(String),
}

/// Authorization gate for MCP server requests.
///
/// Implementations must be cheap to call on each request; long-running
/// permission lookups should be cached at construction time. Implementations
/// MUST NOT mutate any persistent state from inside `authorize`; recording
/// the decision is the caller's responsibility (via `AuditSink`).
#[async_trait]
pub trait McpAuthorizer: Send + Sync {
    async fn authorize(
        &self,
        principal: &PrincipalContext,
        operation: McpOperation<'_>,
    ) -> Result<AuthzDecision, AuthzError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_labels_are_stable() {
        assert_eq!(McpOperation::ListTools.label(), "tools/list");
        assert_eq!(McpOperation::ListResources.label(), "resources/list");
        assert_eq!(
            McpOperation::ListResourceTemplates.label(),
            "resources/templates/list"
        );
        assert_eq!(
            McpOperation::ReadResource {
                uri: "libra://history/latest"
            }
            .label(),
            "resources/read"
        );
        assert_eq!(
            McpOperation::CallTool {
                tool_name: "create_intent"
            }
            .label(),
            "tools/call"
        );
    }

    #[test]
    fn allow_decision_is_allowed_and_has_no_reason() {
        let decision = AuthzDecision::Allow;
        assert!(decision.is_allowed());
        assert!(decision.reason().is_none());
    }

    #[test]
    fn deny_decision_carries_reason_and_is_not_allowed() {
        let decision = AuthzDecision::Deny {
            reason: "principal lacks tools/call scope".to_string(),
        };
        assert!(!decision.is_allowed());
        assert_eq!(decision.reason(), Some("principal lacks tools/call scope"));
    }

    #[test]
    fn needs_human_decision_is_not_allowed_but_carries_reason() {
        let decision = AuthzDecision::NeedsHuman {
            reason: "destructive tool call requires confirmation".to_string(),
        };
        assert!(!decision.is_allowed());
        assert_eq!(
            decision.reason(),
            Some("destructive tool call requires confirmation")
        );
    }

    // The trait is dyn-compatible: a `Box<dyn McpAuthorizer>` must compile.
    // We don't run it; we only need the type-system check.
    #[allow(dead_code)]
    fn _dyn_compat(_authz: Box<dyn McpAuthorizer>) {}
}
