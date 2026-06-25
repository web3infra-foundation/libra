use std::fs;

use git_internal::errors::GitError;
use serial_test::serial;
use tempfile::tempdir;

use super::{
    LsRemoteArgs, LsRemoteError,
    ls_remote_filter::{CompiledPattern, include_reference},
    ls_remote_redaction::{
        sanitize_discovery_error, sanitize_remote_error_reason, visible_remote_display,
        visible_remote_url,
    },
    resolve_remote,
};
use crate::{
    internal::protocol::DiscRef,
    utils::{test::ChangeDirGuard, util},
};

#[test]
fn ls_remote_error_display_pins_each_owned_variant() {
    assert_eq!(
        LsRemoteError::ConfigRead("db locked".to_string()).to_string(),
        "failed to read remote configuration: db locked",
    );
    assert_eq!(
        LsRemoteError::InvalidRemote {
            spec: "ftp://example.com/repo".to_string(),
            reason: "unsupported scheme".to_string(),
        }
        .to_string(),
        "invalid remote 'ftp://example.com/repo': unsupported scheme",
    );
    assert_eq!(
        LsRemoteError::InvalidPattern {
            pattern: "**".to_string(),
            reason: "empty alternation".to_string(),
        }
        .to_string(),
        "invalid ref pattern '**': empty alternation",
    );
    assert_eq!(
        LsRemoteError::UnsupportedSortKey("unknown".to_string()).to_string(),
        "unsupported ls-remote sort key 'unknown'",
    );
}

fn disc_ref(refname: &str) -> DiscRef {
    DiscRef {
        _hash: "1111111111111111111111111111111111111111".to_string(),
        _ref: refname.to_string(),
    }
}

fn args_with_filters(heads: bool, tags: bool, refs: bool) -> LsRemoteArgs {
    LsRemoteArgs {
        heads,
        tags,
        refs,
        symref: false,
        get_url: false,
        exit_code: false,
        sort: None,
        repository: "origin".to_string(),
        patterns: vec![],
    }
}

#[test]
fn plain_pattern_matches_ref_tail() {
    let pattern = CompiledPattern::new("main").unwrap();
    assert!(pattern.matches("refs/heads/main"));
    assert!(!pattern.matches("refs/heads/feature"));
}

#[test]
fn glob_pattern_matches_nested_refs_across_slashes() {
    let full_ref = CompiledPattern::new("refs/heads/*").unwrap();
    assert!(full_ref.matches("refs/heads/feature/foo"));
    assert!(!full_ref.matches("refs/tags/feature/foo"));

    let tail_ref = CompiledPattern::new("feature*").unwrap();
    assert!(tail_ref.matches("refs/heads/feature/foo"));

    let question_ref = CompiledPattern::new("a?b").unwrap();
    assert!(question_ref.matches("refs/heads/a/b"));
}

#[test]
fn refs_flag_excludes_head_and_peeled_tags() {
    let args = args_with_filters(false, false, true);
    assert!(!include_reference(&disc_ref("HEAD"), &args, &[]));
    assert!(!include_reference(
        &disc_ref("refs/tags/v1.0^{}"),
        &args,
        &[]
    ));
    assert!(include_reference(&disc_ref("refs/tags/v1.0"), &args, &[]));
}

#[test]
fn heads_and_tags_filters_use_union() {
    let args = args_with_filters(true, true, false);
    assert!(include_reference(&disc_ref("refs/heads/main"), &args, &[]));
    assert!(include_reference(&disc_ref("refs/tags/v1.0"), &args, &[]));
    assert!(!include_reference(&disc_ref("HEAD"), &args, &[]));
}

#[test]
fn visible_remote_url_redacts_http_credentials() {
    assert_eq!(
        visible_remote_url("https://token@example.com/repo.git"),
        "https://example.com/repo.git"
    );
    assert_eq!(
        visible_remote_url("https://user:secret@example.com/repo.git"),
        "https://example.com/repo.git"
    );
}

#[test]
fn visible_remote_url_redacts_scp_password() {
    assert_eq!(
        visible_remote_url("user:secret@example.com:repo.git"),
        "[REDACTED]@example.com:repo.git"
    );
}

#[tokio::test]
#[serial]
async fn resolve_direct_url_skips_broken_current_repo_config() {
    let repo = tempdir().unwrap();
    let storage = repo.path().join(util::ROOT_DIR);
    fs::create_dir_all(&storage).unwrap();
    fs::write(storage.join(util::DATABASE), b"not sqlite").unwrap();
    let _guard = ChangeDirGuard::new(repo.path());

    let resolved = resolve_remote("https://example.com/repo.git")
        .await
        .unwrap();

    assert_eq!(
        resolved,
        (
            "https://example.com/repo.git".to_string(),
            "https://example.com/repo.git".to_string(),
            None
        )
    );
}

#[test]
fn visible_remote_display_redacts_direct_url_but_preserves_remote_name() {
    assert_eq!(
        visible_remote_display("https://token@example.com/repo.git", None),
        "https://example.com/repo.git"
    );
    assert_eq!(visible_remote_display("origin", Some("origin")), "origin");
}

#[test]
fn visible_remote_display_redacts_direct_scp_password() {
    assert_eq!(
        visible_remote_display("user:secret@example.com:repo.git", None),
        "[REDACTED]@example.com:repo.git"
    );
    assert_eq!(
        visible_remote_display("user:secret@example.com:repo.git", Some("origin")),
        "user:secret@example.com:repo.git"
    );
}

#[test]
fn invalid_remote_reason_redacts_valid_url_credentials() {
    let remote = "file://user:secret@example.com/repo.git";
    let reason = format!("invalid file url: {remote}");

    let sanitized = sanitize_remote_error_reason(&reason, remote);

    assert!(!sanitized.contains("user"));
    assert!(!sanitized.contains("secret"));
    assert!(sanitized.contains("file://example.com/repo.git"));
}

#[test]
fn invalid_remote_reason_redacts_malformed_url_like_credentials() {
    let remote = "https://user:secret@";
    let reason = format!("invalid local repository '{remote}': not found");

    let sanitized = sanitize_remote_error_reason(&reason, remote);

    assert!(!sanitized.contains("user"));
    assert!(!sanitized.contains("secret"));
    assert!(sanitized.contains("https://[REDACTED]@"));
}

#[test]
fn invalid_remote_reason_redacts_scp_like_password_credentials() {
    let remote = "user:secret@example.com:repo.git";
    let reason = format!("invalid local repository '{remote}': not found");

    let sanitized = sanitize_remote_error_reason(&reason, remote);

    assert!(!sanitized.contains("user:secret"));
    assert!(sanitized.contains("[REDACTED]@example.com:repo.git"));
}

#[test]
fn discovery_error_redacts_url_credentials_in_source() {
    let remote = "https://user:secret@example.invalid/repo.git";
    let source = GitError::NetworkError(format!(
        "Failed to send request: error sending request for url ({remote}/info/refs?service=git-upload-pack): dns error"
    ));

    let sanitized = sanitize_discovery_error(source, remote).to_string();

    assert!(!sanitized.contains("user"));
    assert!(!sanitized.contains("secret"));
    assert!(sanitized.contains("https://example.invalid/repo.git"));
}
