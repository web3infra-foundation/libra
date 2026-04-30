use chrono::Utc;
use libra::internal::ai::web::code_ui::{
    CodeUiCapabilities, CodeUiControllerKind, CodeUiInitialController, CodeUiInteractionKind,
    CodeUiInteractionRequest, CodeUiInteractionStatus, CodeUiProviderInfo, CodeUiRuntimeHandle,
    CodeUiSession, ReadOnlyCodeUiAdapter, initial_snapshot,
};

#[tokio::test]
async fn diagnostics_redaction_test() {
    let session = CodeUiSession::new(initial_snapshot(
        "/tmp/libra-diagnostics",
        CodeUiProviderInfo {
            provider: "test".to_string(),
            model: Some("diagnostics-model".to_string()),
            mode: Some("test".to_string()),
            managed: false,
        },
        CodeUiCapabilities::default(),
    ));
    session
        .upsert_interaction(CodeUiInteractionRequest {
            id: "interaction-token=interaction-secret".to_string(),
            kind: CodeUiInteractionKind::Approval,
            title: Some("Approve command".to_string()),
            status: CodeUiInteractionStatus::Pending,
            requested_at: Utc::now(),
            ..CodeUiInteractionRequest::default()
        })
        .await;

    let runtime = CodeUiRuntimeHandle::build_with_control(
        ReadOnlyCodeUiAdapter::new(session, CodeUiCapabilities::default()),
        false,
        true,
        CodeUiInitialController::Unclaimed,
    )
    .await;
    let attach = runtime
        .attach_controller(
            CodeUiControllerKind::Automation,
            "diagnostics-client token=controller-owner-secret",
        )
        .await
        .expect("automation controller should attach");
    runtime
        .ensure_controller_write_access(Some(&attach.controller_token))
        .await
        .expect("controller token should authorize diagnostics refresh");

    let diagnostics = runtime.diagnostics().await;
    let serialized =
        serde_json::to_string(&diagnostics).expect("diagnostics should serialize to JSON");

    assert_eq!(diagnostics.provider, "test");
    assert_eq!(diagnostics.model.as_deref(), Some("diagnostics-model"));
    assert_eq!(
        diagnostics.controller.kind,
        CodeUiControllerKind::Automation
    );
    assert!(
        diagnostics.controller.lease_expires_at.is_some(),
        "automation diagnostics should expose lease timing without exposing tokens"
    );
    assert!(
        diagnostics
            .controller
            .owner_label
            .as_deref()
            .is_some_and(|owner| owner.contains("token=[REDACTED]"))
    );
    assert!(
        diagnostics
            .active_interaction_id
            .as_deref()
            .is_some_and(|id| id.contains("token=[REDACTED]"))
    );
    assert!(!serialized.contains(&attach.controller_token));
    assert!(!serialized.contains("controller-owner-secret"));
    assert!(!serialized.contains("interaction-secret"));
    assert!(!serialized.contains("X-Libra-Control-Token"));
    assert!(!serialized.contains("X-Code-Controller-Token"));
}
