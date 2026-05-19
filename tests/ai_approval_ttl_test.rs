//! CEX-11 approval TTL and canonical key contract tests.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use chrono::{TimeZone, Utc};
use libra::internal::ai::sandbox::{
    ApprovalCachePolicy, ApprovalScope, ApprovalSensitivityTier, ApprovalStore, AskForApproval,
    ExecApprovalRequest, ReviewDecision, SandboxPermissions, ToolApprovalContext,
    request_cached_approval_with_keys, shell_approval_key, shell_approval_key_with_scope,
};
use tokio::sync::{Mutex, mpsc::error::TryRecvError};

#[test]
fn canonical_shell_key_is_stable_for_flag_order_and_scope_fields() {
    let first = shell_approval_key(
        "cargo test --features test-provider --all-targets",
        Path::new("/workspace"),
        SandboxPermissions::UseDefault,
    );
    let second = shell_approval_key(
        "cargo test --all-targets --features test-provider",
        Path::new("/workspace"),
        SandboxPermissions::UseDefault,
    );
    let other_cwd = shell_approval_key(
        "cargo test --all-targets --features test-provider",
        Path::new("/other"),
        SandboxPermissions::UseDefault,
    );
    let escalated = shell_approval_key(
        "cargo test --all-targets --features test-provider",
        Path::new("/workspace"),
        SandboxPermissions::RequireEscalated,
    );

    assert_eq!(first, second);
    assert_ne!(first, other_cwd);
    assert_ne!(first, escalated);
    assert_eq!(first.len(), 64);
    assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
}

#[test]
fn approval_store_expires_ttl_memos_and_keeps_session_memos() {
    let now = Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap();
    let mut store = ApprovalStore::default();

    store.put_ttl(
        "ttl-key".to_string(),
        ReviewDecision::ApprovedForTtl,
        ApprovalScope::Session,
        ApprovalSensitivityTier::Strict,
        now,
        Duration::from_secs(60),
    );
    store.put(
        "session-key".to_string(),
        ReviewDecision::ApprovedForSession,
    );

    assert_eq!(
        store.get_at("ttl-key", now + chrono::Duration::seconds(59)),
        Some(ReviewDecision::ApprovedForTtl)
    );
    assert_eq!(
        store.get_at("ttl-key", now + chrono::Duration::seconds(61)),
        None
    );
    assert_eq!(
        store.get_at("session-key", now + chrono::Duration::days(30)),
        Some(ReviewDecision::ApprovedForSession)
    );
}

#[test]
fn approval_key_changes_when_scope_or_sensitivity_tier_changes() {
    let strict_session = shell_approval_key_with_scope(
        "cargo test --all-targets",
        Path::new("/workspace"),
        SandboxPermissions::UseDefault,
        ApprovalScope::Session,
        ApprovalSensitivityTier::Strict,
    );
    let directory_session = shell_approval_key_with_scope(
        "cargo test --all-targets",
        Path::new("/workspace"),
        SandboxPermissions::UseDefault,
        ApprovalScope::Session,
        ApprovalSensitivityTier::Directory,
    );
    let pattern_project = shell_approval_key_with_scope(
        "cargo test --all-targets",
        Path::new("/workspace"),
        SandboxPermissions::UseDefault,
        ApprovalScope::Project,
        ApprovalSensitivityTier::Pattern,
    );

    assert_ne!(strict_session, directory_session);
    assert_ne!(directory_session, pattern_project);
    assert_ne!(strict_session, pattern_project);
}

#[test]
fn denied_approval_decisions_are_not_cached() {
    let mut store = ApprovalStore::default();
    store.put("denied".to_string(), ReviewDecision::Denied);

    assert_eq!(store.get("denied"), None);
}

#[test]
fn approval_store_revocation_removes_active_memo() {
    let now = Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap();
    let mut store = ApprovalStore::default();
    store.put_ttl(
        "key".to_string(),
        ReviewDecision::ApprovedForTtl,
        ApprovalScope::Project,
        ApprovalSensitivityTier::Directory,
        now,
        Duration::from_secs(300),
    );

    assert!(store.revoke("key"));
    assert_eq!(store.get_at("key", now), None);
    assert!(!store.revoke("key"));
}

#[test]
fn approval_store_allow_all_can_be_revoked_per_scope() {
    let mut store = ApprovalStore::default();
    store.approve_all_commands_for_scope("automation:turn-1");
    store.approve_all_commands();

    assert!(store.allow_all_commands());
    assert!(store.allow_all_commands_for_scope("automation:turn-1"));
    assert_eq!(store.active_allow_all_scopes().len(), 2);

    assert!(store.revoke_allow_all_for_scope("automation:turn-1"));
    assert!(!store.allow_all_commands_for_scope("automation:turn-1"));
    assert!(store.allow_all_commands(), "default scope should remain");
    assert!(!store.revoke_allow_all_for_scope("automation:turn-1"));
}

#[test]
fn approval_store_overflowing_ttl_falls_back_to_one_week_cap() {
    let now = Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap();
    let mut store = ApprovalStore::default();
    // Pathological caller passes a TTL that overflows chrono::Duration's
    // wall-clock arithmetic — the memo must still expire via the 7-day
    // fallback rather than silently becoming session-permanent.
    store.put_ttl(
        "huge".to_string(),
        ReviewDecision::ApprovedForTtl,
        ApprovalScope::Session,
        ApprovalSensitivityTier::Strict,
        now,
        Duration::MAX,
    );

    let inside_cap = now + chrono::Duration::hours(1);
    let beyond_cap = now + chrono::Duration::days(7) + chrono::Duration::seconds(1);
    assert!(
        store.get_at("huge", inside_cap).is_some(),
        "TTL fallback memo must remain active well within the cap"
    );
    assert_eq!(
        store.get_at("huge", beyond_cap),
        None,
        "TTL fallback must not exceed the 7-day safety cap"
    );
}

#[test]
fn approval_store_honest_long_ttl_is_not_silently_capped() {
    let now = Utc.with_ymd_and_hms(2026, 5, 3, 12, 0, 0).unwrap();
    let mut store = ApprovalStore::default();
    // A user-configured 14-day TTL is within chrono::Duration's range and
    // must be honoured — only the overflow path falls back to the 7-day cap.
    store.put_ttl(
        "long".to_string(),
        ReviewDecision::ApprovedForTtl,
        ApprovalScope::Session,
        ApprovalSensitivityTier::Strict,
        now,
        Duration::from_secs(60 * 60 * 24 * 14),
    );

    let twelve_days = now + chrono::Duration::days(12);
    assert!(
        store.get_at("long", twelve_days).is_some(),
        "honest 14-day TTL must remain active at 12 days, not silently expire at 7"
    );
}

#[tokio::test]
async fn ttl_approval_skips_second_prompt_within_ttl() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let store = Arc::new(Mutex::new(ApprovalStore::default()));
    let ctx = ToolApprovalContext {
        policy: AskForApproval::OnRequest,
        request_tx: tx,
        store: Arc::clone(&store),
        scope_key_prefix: None,
        approval_ttl: Duration::from_secs(60),
        cache_policy: ApprovalCachePolicy::default(),
    };
    let keys = vec![shell_approval_key(
        "cargo test --features test-provider --all-targets",
        Path::new("/workspace"),
        SandboxPermissions::UseDefault,
    )];

    let responder = tokio::spawn(async move {
        let request = rx.recv().await.expect("approval request expected");
        let _ = request.response_tx.send(ReviewDecision::ApprovedForTtl);
        rx
    });

    let first = request_cached_approval_with_keys(&ctx, &keys, |response_tx| {
        test_approval_request(response_tx)
    })
    .await;
    assert_eq!(first, ReviewDecision::ApprovedForTtl);

    let mut rx = responder.await.expect("responder task failed");
    let second = request_cached_approval_with_keys(&ctx, &keys, |response_tx| {
        test_approval_request(response_tx)
    })
    .await;

    assert_eq!(second, ReviewDecision::ApprovedForTtl);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
}

fn test_approval_request(
    response_tx: tokio::sync::oneshot::Sender<ReviewDecision>,
) -> ExecApprovalRequest {
    ExecApprovalRequest {
        call_id: "call-ttl".to_string(),
        command: "cargo test --all-targets --features test-provider".to_string(),
        cwd: PathBuf::from("/workspace"),
        reason: None,
        is_retry: false,
        sandbox_label: "workspace-write".to_string(),
        network_access: false,
        writable_roots: vec![PathBuf::from("/workspace")],
        cache_disabled_reason: None,
        response_tx,
    }
}
