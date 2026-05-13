use std::path::Path;

use libra::internal::publish::preflight::{DenyReason, Preflight, PreflightDecision};

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
