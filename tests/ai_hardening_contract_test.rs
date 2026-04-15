//! Phase E hardening contract tests.

use chrono::Utc;
use libra::internal::ai::runtime::{
    AuditEvent, AuditSink, InMemoryAuditSink, PrincipalContext, PrincipalRole, SecretRedactor,
    ToolBoundaryPolicy, ToolOperation,
};
use uuid::Uuid;

#[test]
fn tool_boundary_blocks_observer_mutations_and_network_by_default() {
    let policy = ToolBoundaryPolicy::default_runtime();
    let observer = PrincipalContext {
        principal_id: "browser".to_string(),
        role: PrincipalRole::Observer,
    };

    let mutation = policy.decide(
        &observer,
        &ToolOperation {
            tool_name: "apply_patch".to_string(),
            mutates_state: true,
            requires_network: false,
        },
    );
    assert!(!mutation.allowed);
    assert!(!mutation.approval_required);

    let network_read = policy.decide(
        &PrincipalContext::system(),
        &ToolOperation {
            tool_name: "read_file".to_string(),
            mutates_state: false,
            requires_network: true,
        },
    );
    assert!(!network_read.allowed);
}

#[test]
fn mutating_tool_requires_runtime_mediated_approval_for_humans() {
    let policy = ToolBoundaryPolicy::default_runtime();
    let human = PrincipalContext {
        principal_id: "alice".to_string(),
        role: PrincipalRole::Owner,
    };

    let decision = policy.decide(
        &human,
        &ToolOperation {
            tool_name: "shell".to_string(),
            mutates_state: true,
            requires_network: false,
        },
    );

    assert!(decision.allowed);
    assert!(decision.approval_required);
}

#[test]
fn mcp_read_and_write_tool_prefixes_are_classified() {
    let policy = ToolBoundaryPolicy::default_runtime();

    let list_decisions = policy.decide(
        &PrincipalContext::system(),
        &ToolOperation {
            tool_name: "list_decisions".to_string(),
            mutates_state: false,
            requires_network: false,
        },
    );
    assert!(list_decisions.allowed);
    assert!(!list_decisions.approval_required);

    let create_decision = policy.decide(
        &PrincipalContext {
            principal_id: "alice".to_string(),
            role: PrincipalRole::Owner,
        },
        &ToolOperation {
            tool_name: "create_decision".to_string(),
            mutates_state: true,
            requires_network: false,
        },
    );
    assert!(create_decision.allowed);
    assert!(create_decision.approval_required);
}

#[test]
fn secret_redactor_removes_common_token_shapes() {
    let redactor = SecretRedactor::default_runtime();
    let redacted = redactor.redact(
        "OPENAI_API_KEY=sk-live Authorization: Bearer abc123 password: hunter2 token=secret",
    );

    assert!(!redacted.contains("sk-live"));
    assert!(!redacted.contains("abc123"));
    assert!(!redacted.contains("hunter2"));
    assert!(!redacted.contains("secret"));
    assert_eq!(redacted.matches("[REDACTED]").count(), 4);
}

#[tokio::test]
async fn audit_sink_records_redacted_policy_events() {
    let sink = InMemoryAuditSink::default();
    let policy = ToolBoundaryPolicy::default_runtime();
    let redactor = SecretRedactor::default_runtime();

    sink.append(AuditEvent {
        trace_id: Uuid::new_v4(),
        principal_id: "alice".to_string(),
        action: "tool_call".to_string(),
        policy_version: policy.policy_version().to_string(),
        redacted_summary: redactor.redact("token=secret shell apply_patch"),
        at: Utc::now(),
    })
    .await
    .unwrap();
    sink.flush().await.unwrap();

    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].policy_version, "tool-boundary:v1");
    assert!(!events[0].redacted_summary.contains("secret"));
}
