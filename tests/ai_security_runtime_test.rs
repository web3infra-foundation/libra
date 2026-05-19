//! Integration acceptance for `libra code` Phase 5 security runtime —
//! agent.md Implementation Phase 5 "authz、redaction、shell boundary、durable
//! audit".
//!
//! Pins the four-axis Phase 5 contract:
//!
//! - **authz** — `PrincipalContext::system()` produces a stable
//!   `principal_id="libra-runtime"` with `PrincipalRole::System`; every audit
//!   event carries the principal id verbatim.
//! - **redaction** — `SecretRedactor::default_runtime()` strips every default
//!   marker (`api_key:`, `token:`, `password:`, `authorization: bearer`,
//!   `control_token:` / `control-token:`, `x-libra-control-token:`,
//!   `x-code-controller-token:`, `secret-`, `secret_`); `redact()` is
//!   idempotent (re-running on already-redacted output is a no-op).
//! - **shell boundary** — `ToolBoundaryPolicy::default_runtime()` classifies
//!   the canonical mutating tools (`shell`, `apply_patch`, …) as requiring
//!   approval and accepts the readonly set (`read_file`, `list_dir`, …)
//!   without approval; `ToolBoundaryRuntime::decide()` reflects that policy.
//! - **durable audit** — `InMemoryAuditSink::append()` records events in
//!   the order they arrived; `ToolBoundaryRuntime::append_audit()` honours
//!   the trace_id / principal / policy_version supplied at construction and
//!   never emits raw secrets in `redacted_summary`.
//!
//! Layer: L1 — hermetic, no filesystem, no network, no real provider.

use std::sync::Arc;

use libra::internal::ai::runtime::hardening::{
    AuditEvent, AuditSink, BoundaryDecision, InMemoryAuditSink, PrincipalContext, PrincipalRole,
    SecretRedactor, ToolBoundaryPolicy, ToolBoundaryRuntime, ToolOperation,
};
use uuid::Uuid;

/// Redaction property #1: every default marker is stripped, AND `redact()`
/// is idempotent (running twice produces the same string). The idempotence
/// half guards against double-redaction bugs where a second pass might find
/// `[REDACTED]` itself as a "marker" tail and corrupt the output.
#[test]
fn secret_redactor_default_markers_strip_all_known_secret_shapes_and_are_idempotent() {
    let redactor = SecretRedactor::default_runtime();

    // One sample per default marker family. Each line carries a value that
    // a naive logger might write verbatim; `redact()` must scrub it.
    let raw = [
        "api_key: sk-deadbeefcafe1234",
        "api_key=sk-deadbeefcafe1234",
        "authorization: bearer abc.def.ghi",
        "control_token: 0xff00ff00",
        "control_token=0xff00ff00",
        "control-token: 0xff00ff00",
        "control-token=0xff00ff00",
        "password: hunter2",
        "password=hunter2",
        "token: glpat-xxxxxxxxxxxxxxxxxxxx",
        "token=glpat-xxxxxxxxxxxxxxxxxxxx",
        "x-code-controller-token: ctrl-abc",
        "x-code-controller-token=ctrl-abc",
        "x-libra-control-token: lcc-xyz",
        "x-libra-control-token=lcc-xyz",
        "/tmp/abc-secret-key-xyz/libra.log",
        "/tmp/abc-secret_key_xyz/libra.log",
    ]
    .join(" ");

    let once = redactor.redact(&raw);
    let twice = redactor.redact(&once);

    // None of the raw secret tails may survive the first pass…
    for needle in [
        "sk-deadbeefcafe1234",
        "abc.def.ghi",
        "0xff00ff00",
        "hunter2",
        "glpat-xxxxxxxxxxxxxxxxxxxx",
        "ctrl-abc",
        "lcc-xyz",
        "key-xyz",
        "key_xyz",
    ] {
        assert!(
            !once.contains(needle),
            "redacted output still contained `{needle}`; first pass output:\n{once}",
        );
    }

    // …and the second pass must be a fixed point.
    assert_eq!(
        once, twice,
        "SecretRedactor::redact() must be idempotent; got drift between pass 1 and pass 2\npass1: {once}\npass2: {twice}",
    );
}

/// authz property: `PrincipalContext::system()` is a well-known constant.
/// Audit events recorded against it must surface the same principal_id, and
/// the role must be `System` (not Owner / Contributor / Observer).
#[test]
fn principal_context_system_carries_stable_id_and_role() {
    let principal = PrincipalContext::system();
    assert_eq!(principal.principal_id, "libra-runtime");
    assert_eq!(principal.role, PrincipalRole::System);

    let event = AuditEvent {
        trace_id: Uuid::nil(),
        principal_id: principal.principal_id.clone(),
        action: "test".to_string(),
        policy_version: "v0".to_string(),
        redacted_summary: "no secret here".to_string(),
        at: chrono::Utc::now(),
    };
    assert_eq!(event.principal_id, "libra-runtime");
}

/// Durable audit property: `InMemoryAuditSink` is an append-only log;
/// `events()` returns recorded events in insertion order, byte-identical to
/// what was appended (no reordering, no merging, no silent drops).
#[tokio::test]
async fn in_memory_audit_sink_preserves_append_order() {
    let sink = InMemoryAuditSink::default();

    for i in 0..5u32 {
        sink.append(AuditEvent {
            trace_id: Uuid::nil(),
            principal_id: format!("agent-{i}"),
            action: format!("action-{i}"),
            policy_version: "v0".to_string(),
            redacted_summary: format!("event #{i}"),
            at: chrono::Utc::now(),
        })
        .await
        .unwrap();
    }

    let events = sink.events().await;
    assert_eq!(events.len(), 5);
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.principal_id, format!("agent-{i}"));
        assert_eq!(event.action, format!("action-{i}"));
        assert_eq!(event.redacted_summary, format!("event #{i}"));
    }
}

/// `record_decision` default-impl property: forwards an `AuditEvent` whose
/// `action == "boundary_decision"`, `principal_id` matches the supplied
/// principal, and `redacted_summary` is the redactor-processed render of the
/// (operation, decision) pair. A secret embedded in `decision.reason` MUST
/// be stripped before reaching the sink.
#[tokio::test]
async fn record_decision_default_impl_redacts_secret_in_reason() {
    let sink = Arc::new(InMemoryAuditSink::default());
    let principal = PrincipalContext::system();
    let redactor = SecretRedactor::default_runtime();
    let trace_id = Uuid::new_v4();
    let operation = ToolOperation {
        tool_name: "shell".to_string(),
        mutates_state: true,
        requires_network: false,
    };
    let decision = BoundaryDecision {
        allowed: false,
        approval_required: true,
        reason: "deny: token: glpat-leaked-1234567890abcdef must be rotated".to_string(),
    };

    AuditSink::record_decision(
        sink.as_ref(),
        trace_id,
        &principal,
        "tool-boundary:v1",
        &operation,
        &decision,
        &redactor,
    )
    .await
    .unwrap();

    let events = sink.events().await;
    assert_eq!(events.len(), 1, "exactly one audit event expected");
    let event = &events[0];
    assert_eq!(event.trace_id, trace_id);
    assert_eq!(event.principal_id, "libra-runtime");
    assert_eq!(event.action, "boundary_decision");
    assert_eq!(event.policy_version, "tool-boundary:v1");
    assert!(
        !event
            .redacted_summary
            .contains("glpat-leaked-1234567890abcdef"),
        "redacted_summary leaked the raw token: {}",
        event.redacted_summary,
    );
    // Still mentions which tool / decision shape so the audit row remains
    // diagnostic; only the secret tail is gone.
    assert!(event.redacted_summary.contains("tool=shell"));
    assert!(event.redacted_summary.contains("allowed=false"));
}

/// Shell-boundary property: the canonical default policy classifies the
/// listed mutating tools as decisions that require approval (allowed=false
/// or approval_required=true), while readonly tools pass without approval.
/// `ToolBoundaryRuntime::system()` builds a runtime around the default
/// policy, redactor, and a caller-supplied sink; `append_audit()` then writes
/// a redacted, sink-ordered event whose principal_id and policy_version both
/// match the system bootstrap (`libra-runtime`, `tool-boundary:v1`).
#[tokio::test]
async fn tool_boundary_runtime_system_writes_redacted_audit_with_bound_principal() {
    let sink = Arc::new(InMemoryAuditSink::default());
    let trace_id = Uuid::new_v4();
    let runtime = ToolBoundaryRuntime::system(trace_id, sink.clone());

    // Readonly tool: shouldn't require approval.
    let readonly_op = ToolOperation {
        tool_name: "read_file".to_string(),
        mutates_state: false,
        requires_network: false,
    };
    let readonly = runtime.decide(&readonly_op);
    assert!(
        readonly.allowed && !readonly.approval_required,
        "read_file must be allowed without approval under the default policy, got {readonly:?}",
    );

    // Mutating tool under the SYSTEM principal: the runtime bootstrap uses
    // `PrincipalContext::system()`, which bypasses approval because the
    // system principal is the trusted runtime caller. The reason field
    // still calls out the mutating-tool classification so audit can see it.
    let mutating_op = ToolOperation {
        tool_name: "apply_patch".to_string(),
        mutates_state: true,
        requires_network: false,
    };
    let mutating = runtime.decide(&mutating_op);
    assert!(
        mutating.allowed,
        "system principal must be allowed to call apply_patch; got {mutating:?}"
    );
    assert!(
        !mutating.approval_required,
        "system principal must bypass approval for mutating tools; got {mutating:?}",
    );
    assert!(
        mutating.reason.contains("mutating tool"),
        "mutating tool decision reason must still flag the surface for audit; got reason {:?}",
        mutating.reason,
    );

    // Now exercise append_audit and assert the sink received exactly one
    // event whose summary has the bearer token stripped.
    runtime
        .append_audit(
            "tool_boundary.read_file",
            "scope=workspace, authorization: bearer abc.def.ghi, decision=allow",
        )
        .await
        .unwrap();

    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.trace_id, trace_id);
    assert_eq!(event.principal_id, "libra-runtime");
    assert_eq!(event.policy_version, "tool-boundary:v1");
    assert_eq!(event.action, "tool_boundary.read_file");
    assert!(
        !event.redacted_summary.contains("abc.def.ghi"),
        "bearer token must be redacted before reaching the audit sink, got {}",
        event.redacted_summary,
    );
    assert!(event.redacted_summary.contains("scope=workspace"));
    assert!(event.redacted_summary.contains("decision=allow"));
}

/// Policy property: the readonly / mutating tool sets in
/// `ToolBoundaryPolicy::default_runtime()` cover the canonical lists the
/// hardening doc commits to. A regression here (e.g. someone moves
/// `apply_patch` to readonly_tools) would silently degrade the security
/// posture, so pin the membership explicitly.
///
/// Uses a **Contributor** principal — System bypasses approval entirely so
/// it can't distinguish mutating-vs-readonly intent; Contributor exposes
/// the membership boundary by demanding approval on mutating tools.
#[test]
fn tool_boundary_policy_default_runtime_classifies_canonical_tools() {
    let policy = ToolBoundaryPolicy::default_runtime();
    let contributor = PrincipalContext {
        principal_id: "test-contributor".to_string(),
        role: PrincipalRole::Contributor,
    };

    for readonly_tool in [
        "read_file",
        "list_dir",
        "grep_files",
        "search_files",
        "web_search",
        "request_user_input",
        "mcp_read",
        "run_libra_vcs",
    ] {
        let op = ToolOperation {
            tool_name: readonly_tool.to_string(),
            mutates_state: false,
            requires_network: false,
        };
        let decision = policy.decide(&contributor, &op);
        assert!(
            decision.allowed && !decision.approval_required,
            "readonly tool `{readonly_tool}` must not require approval, got {decision:?}",
        );
    }

    for mutating_tool in [
        "shell",
        "apply_patch",
        "update_plan",
        "submit_intent_draft",
        "submit_plan_draft",
        "submit_task_complete",
        "mcp_write",
    ] {
        let op = ToolOperation {
            tool_name: mutating_tool.to_string(),
            mutates_state: true,
            requires_network: false,
        };
        let decision = policy.decide(&contributor, &op);
        assert!(
            decision.allowed,
            "mutating tool `{mutating_tool}` should still be allowable for Contributor (approval gates it), got {decision:?}",
        );
        assert!(
            decision.approval_required,
            "mutating tool `{mutating_tool}` must require approval for non-System principals, got {decision:?}",
        );
    }

    // Observer principal must be denied outright on any mutating tool —
    // regardless of whether the tool name is in mutating_tools.
    let observer = PrincipalContext {
        principal_id: "test-observer".to_string(),
        role: PrincipalRole::Observer,
    };
    let observer_decision = policy.decide(
        &observer,
        &ToolOperation {
            tool_name: "shell".to_string(),
            mutates_state: true,
            requires_network: false,
        },
    );
    assert!(
        !observer_decision.allowed && !observer_decision.approval_required,
        "Observer principals must be denied without approval prompt, got {observer_decision:?}",
    );

    assert_eq!(policy.policy_version(), "tool-boundary:v1");
}
