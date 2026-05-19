use std::path::Path;

use libra::internal::publish::{
    contract::SiteVisibility,
    preflight::{DenyReason, Preflight, PreflightDecision, PreflightPolicyError},
};

#[test]
fn publish_preflight_test_builtin_sensitive_paths_are_denied() {
    let preflight = Preflight::new();

    assert_eq!(
        preflight.evaluate(Path::new(".env.local"), false),
        PreflightDecision::Deny(DenyReason::BuiltinCredential)
    );
    assert_eq!(
        preflight.evaluate(Path::new("config/private-key.pem"), false),
        PreflightDecision::Deny(DenyReason::BuiltinCredential)
    );
    assert_eq!(
        preflight.evaluate(Path::new(".ssh/id_rsa"), false),
        PreflightDecision::Deny(DenyReason::BuiltinCredential)
    );
}

#[test]
fn publish_preflight_test_user_ignore_rules_are_metadata_only_denies() {
    let mut preflight = Preflight::new();
    preflight.extend_with_ignore_text("*.bak\nsecrets\n!important.bak\n");

    assert_eq!(
        preflight.evaluate(Path::new("notes.bak"), false),
        PreflightDecision::Deny(DenyReason::UserIgnore)
    );
    assert_eq!(
        preflight.evaluate(Path::new("secrets/token.txt"), false),
        PreflightDecision::Deny(DenyReason::UserIgnore)
    );
    assert_eq!(
        preflight.evaluate(Path::new("important.bak"), false),
        PreflightDecision::Allow
    );
}

#[test]
fn publish_preflight_test_allow_sensitive_path_overrides_builtin_deny_only_for_exact_path() {
    let preflight = Preflight::new().with_allow_sensitive_paths(vec![".env.local".to_string()]);

    assert_eq!(
        preflight.evaluate(Path::new(".env.local"), false),
        PreflightDecision::Allow
    );
    assert_eq!(
        preflight.evaluate(Path::new(".env.production"), false),
        PreflightDecision::Deny(DenyReason::BuiltinCredential)
    );
}

#[test]
fn publish_preflight_test_public_visibility_rejects_sensitive_allowlist() {
    let err = Preflight::for_visibility(
        SiteVisibility::Public,
        vec![".env.local".to_string(), "keys/server.pem".to_string()],
    )
    .expect_err("public sites must not opt sensitive paths back in");

    assert_eq!(err, PreflightPolicyError::PublicSensitiveAllowlist);
}

#[test]
fn publish_preflight_test_public_visibility_without_allowlist_keeps_default_denies() {
    let preflight = Preflight::for_visibility(SiteVisibility::Public, Vec::new())
        .expect("public sites without allowlist use default policy");

    assert_eq!(
        preflight.evaluate(Path::new(".env.local"), false),
        PreflightDecision::Deny(DenyReason::BuiltinCredential)
    );
}

#[test]
fn publish_preflight_test_private_visibility_honors_exact_sensitive_allowlist() {
    let preflight = Preflight::for_visibility(
        SiteVisibility::Private,
        vec!["config/.env.local".to_string()],
    )
    .expect("private sites may opt in exact sensitive paths");

    assert_eq!(
        preflight.evaluate(Path::new("config/.env.local"), false),
        PreflightDecision::Allow
    );
    assert_eq!(
        preflight.evaluate(Path::new(".env.local"), false),
        PreflightDecision::Deny(DenyReason::BuiltinCredential)
    );
}
