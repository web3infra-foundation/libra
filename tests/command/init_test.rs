//! Integration tests for the `init` command core behavior.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use libra::internal::{config::ConfigKv, db::get_db_conn_instance_for_path, model::config};
use pgp::composed::{Deserializable, SignedPublicKey};
use sea_orm::EntityTrait;
use tempfile::tempdir;

use super::{assert_cli_success, run_libra_command};

async fn open_repo_conn(repo: &std::path::Path, bare: bool) -> sea_orm::DatabaseConnection {
    let db_path = if bare {
        repo.join("libra.db")
    } else {
        repo.join(".libra").join("libra.db")
    };
    get_db_conn_instance_for_path(&db_path)
        .await
        .expect("failed to open repository database")
}

async fn config_value(conn: &sea_orm::DatabaseConnection, key: &str) -> Option<String> {
    ConfigKv::get_with_conn(conn, key)
        .await
        .expect("failed to query config_kv")
        .map(|entry| entry.value)
}

fn public_key_user_ids(public_key: &str) -> Vec<String> {
    let (signed_key, _headers) =
        SignedPublicKey::from_string(public_key).expect("failed to parse armored public key");
    signed_key
        .details
        .users
        .into_iter()
        .map(|user| {
            user.id
                .as_str()
                .expect("public key user id should be valid UTF-8")
                .to_string()
        })
        .collect()
}

fn run_libra_command_with_env(args: &[&str], cwd: &Path, envs: &[(&str, &str)]) -> Output {
    let home = cwd.join(".libra-test-home");
    let config_home = home.join(".config");
    fs::create_dir_all(&config_home).expect("failed to create isolated config directory");

    let mut command = Command::new(env!("CARGO_BIN_EXE_libra"));
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("LIBRA_TEST", "1");

    for (key, value) in envs {
        command.env(key, value);
    }

    command
        .output()
        .expect("failed to execute libra command with extra env")
}

#[tokio::test]
async fn init_vault_false_writes_seed_keys_and_human_summary() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command(&["init", "--vault", "false"], &repo);
    assert_cli_success(&output, "init --vault false");
    assert!(
        repo.join(".libraignore").exists(),
        "non-bare init should create a visible root .libraignore"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("Initialized empty Libra repository in"),
        "expected past-tense success summary, got: {stdout}"
    );
    assert!(
        stderr.contains("Creating repository layout ..."),
        "expected human progress on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("Initializing database ..."),
        "expected database progress on stderr, got: {stderr}"
    );

    let conn = open_repo_conn(&repo, false).await;
    assert_eq!(
        config_value(&conn, "core.repositoryformatversion")
            .await
            .as_deref(),
        Some("0")
    );
    assert_eq!(
        config_value(&conn, "core.filemode").await.as_deref(),
        Some(if cfg!(windows) { "false" } else { "true" })
    );
    assert_eq!(
        config_value(&conn, "core.bare").await.as_deref(),
        Some("false")
    );
    assert_eq!(
        config_value(&conn, "core.logallrefupdates")
            .await
            .as_deref(),
        Some("true")
    );
    assert_eq!(
        config_value(&conn, "core.objectformat").await.as_deref(),
        Some("sha1")
    );
    assert_eq!(
        config_value(&conn, "core.initrefformat").await.as_deref(),
        Some("strict")
    );
    assert_eq!(
        config_value(&conn, "vault.signing").await.as_deref(),
        Some("false")
    );

    let repo_id = config_value(&conn, "libra.repoid")
        .await
        .expect("libra.repoid should exist");
    uuid::Uuid::parse_str(&repo_id).expect("libra.repoid should be a valid UUID");

    let legacy_rows = config::Entity::find()
        .all(&conn)
        .await
        .expect("failed to inspect legacy config table");
    assert!(
        legacy_rows.is_empty(),
        "init should not seed the legacy config table"
    );
}

#[test]
fn init_status_shows_root_libraignore_as_untracked() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command(&["init", "--vault", "false"], &repo);
    assert_cli_success(&output, "init --vault false");

    let status = run_libra_command(&["status", "--short"], &repo);
    assert_cli_success(&status, "status --short");
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("?? .libraignore"),
        "new repository should show .libraignore as an untracked project file, got: {stdout}"
    );
}

#[test]
fn init_preserves_existing_root_libraignore() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join(".libraignore"), "custom-cache/\n").unwrap();

    let output = run_libra_command(&["init", "--vault", "false"], &repo);
    assert_cli_success(&output, "init --vault false");

    let content = fs::read_to_string(repo.join(".libraignore")).unwrap();
    assert_eq!(
        content, "custom-cache/\n",
        "init must not overwrite a user-provided .libraignore"
    );
}

#[test]
fn init_bare_does_not_create_root_libraignore() {
    let temp = tempdir().unwrap();
    let bare_repo = temp.path().join("repo.git");

    let output = run_libra_command(
        &[
            "init",
            "--bare",
            "--vault",
            "false",
            bare_repo.to_str().unwrap(),
        ],
        temp.path(),
    );
    assert_cli_success(&output, "bare init");

    assert!(
        !bare_repo.join(".libraignore").exists(),
        "bare init should not create a worktree .libraignore"
    );
}

#[tokio::test]
async fn init_vault_true_records_signing_state_and_uses_global_identity_fallback() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let set_name = run_libra_command(&["config", "--global", "user.name", "Global Name"], &repo);
    assert_cli_success(&set_name, "set global user.name");
    let set_email = run_libra_command(
        &["config", "--global", "user.email", "global@example.com"],
        &repo,
    );
    assert_cli_success(&set_email, "set global user.email");

    let output = run_libra_command(&["init"], &repo);
    assert_cli_success(&output, "init with global identity fallback");

    let conn = open_repo_conn(&repo, false).await;
    assert_eq!(
        config_value(&conn, "vault.signing").await.as_deref(),
        Some("true")
    );

    let pubkey = config_value(&conn, "vault.gpg.pubkey")
        .await
        .expect("vault.gpg.pubkey should exist after init");
    let user_ids = public_key_user_ids(&pubkey);
    assert!(
        user_ids
            .iter()
            .any(|user_id| user_id == "Global Name <global@example.com>"),
        "expected PGP public key to use global identity, got user IDs: {user_ids:?}"
    );
}

#[tokio::test]
async fn init_vault_true_uses_env_identity_fallback_when_config_is_missing() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command_with_env(
        &["init"],
        &repo,
        &[
            ("GIT_COMMITTER_NAME", "Env Committer"),
            ("EMAIL", "env@example.com"),
        ],
    );
    assert_cli_success(&output, "init with env identity fallback");

    let conn = open_repo_conn(&repo, false).await;
    let pubkey = config_value(&conn, "vault.gpg.pubkey")
        .await
        .expect("vault.gpg.pubkey should exist after init");
    let user_ids = public_key_user_ids(&pubkey);
    assert!(
        user_ids
            .iter()
            .any(|user_id| user_id == "Env Committer <env@example.com>"),
        "expected PGP public key to use env fallback identity, got user IDs: {user_ids:?}"
    );
}

#[tokio::test]
async fn init_target_repo_does_not_inherit_local_identity_from_current_repo() {
    let temp = tempdir().unwrap();
    let repo_a = temp.path().join("repo-a");
    let repo_b = temp.path().join("repo-b");
    fs::create_dir_all(&repo_a).unwrap();

    let init_a = run_libra_command(&["init", "--vault", "false"], &repo_a);
    assert_cli_success(&init_a, "init repo-a");

    let set_name = run_libra_command(&["config", "user.name", "Repo A Name"], &repo_a);
    assert_cli_success(&set_name, "set repo-a local user.name");
    let set_email = run_libra_command(&["config", "user.email", "repo-a@example.com"], &repo_a);
    assert_cli_success(&set_email, "set repo-a local user.email");

    let init_b = run_libra_command_with_env(
        &["init", "../repo-b"],
        &repo_a,
        &[
            ("GIT_COMMITTER_NAME", "Repo B Env"),
            ("EMAIL", "repo-b@example.com"),
        ],
    );
    assert_cli_success(&init_b, "init repo-b from inside repo-a");

    let conn_b = open_repo_conn(&repo_b, false).await;
    let pubkey_b = config_value(&conn_b, "vault.gpg.pubkey")
        .await
        .expect("vault.gpg.pubkey should exist in repo-b");
    let user_ids = public_key_user_ids(&pubkey_b);
    assert!(
        user_ids
            .iter()
            .any(|user_id| user_id == "Repo B Env <repo-b@example.com>"),
        "repo-b should use env/global/default fallback for its own target, got user IDs: {user_ids:?}"
    );
    assert!(
        user_ids
            .iter()
            .all(|user_id| user_id != "Repo A Name <repo-a@example.com>"),
        "repo-b should not inherit repo-a local identity, got user IDs: {user_ids:?}"
    );
}

#[test]
fn init_bare_reinit_returns_repo_state_invalid() {
    let temp = tempdir().unwrap();

    let first = run_libra_command(
        &["init", "--bare", "repo.git", "--vault", "false"],
        temp.path(),
    );
    assert_cli_success(&first, "initial bare init");

    let bare_repo = temp.path().join("repo.git");
    let second = run_libra_command(&["init", "--bare", "--vault", "false"], &bare_repo);
    assert_eq!(second.status.code(), Some(128));

    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("repository already initialized"),
        "expected reinit failure, got: {stderr}"
    );
    assert!(
        stderr.contains("LBR-REPO-003"),
        "expected stable repo-state code, got: {stderr}"
    );
}

#[test]
fn init_worktree_reinit_returns_repo_state_invalid() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let first = run_libra_command(&["init", "--vault", "false"], &repo);
    assert_cli_success(&first, "initial worktree init");

    let second = run_libra_command(&["init", "--vault", "false"], &repo);
    assert_eq!(second.status.code(), Some(128));

    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("repository already initialized"),
        "expected worktree reinit failure, got: {stderr}"
    );
    assert!(
        stderr.contains("remove .libra/ to reinitialize."),
        "expected worktree reinit hint, got: {stderr}"
    );
    assert!(
        stderr.contains("LBR-REPO-003"),
        "expected stable repo-state code, got: {stderr}"
    );
}

#[test]
fn init_invalid_object_format_suggests_sha256() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command(&["init", "--object-format", "sha265"], &repo);
    assert_eq!(output.status.code(), Some(129));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported object format 'sha265'"),
        "expected object-format error, got: {stderr}"
    );
    assert!(
        stderr.contains("did you mean 'sha256'?"),
        "expected fuzzy-match hint, got: {stderr}"
    );
    assert!(
        stderr.contains("LBR-CLI-002"),
        "expected CLI invalid-arguments code, got: {stderr}"
    );
}

#[test]
fn init_vault_true_ignores_commit_use_config_only_strictness() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let set = run_libra_command(&["config", "--global", "user.useConfigOnly", "true"], &repo);
    assert_cli_success(&set, "set user.useConfigOnly");

    let output = run_libra_command(&["init"], &repo);
    assert_cli_success(
        &output,
        "init should still succeed even when user.useConfigOnly=true and identity is missing",
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Generating PGP signing key ..."),
        "expected vault key generation progress, got: {stderr}"
    );
}

// ── `--shared` persistence + vault isolation (init-improvement-plan Batch 1) ──

#[cfg(not(target_os = "windows"))]
fn mode_bits(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("failed to stat {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o7777
}

#[cfg(not(target_os = "windows"))]
#[test]
fn test_shared_group_content_writable() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command(&["init", "--shared=group", "--vault", "false"], &repo);
    assert_cli_success(&output, "init --shared=group");

    let libra = repo.join(".libra");
    assert_eq!(
        mode_bits(&libra.join("objects")) & 0o020,
        0o020,
        "objects must be group-writable under --shared=group"
    );
    assert_eq!(
        mode_bits(&libra) & 0o022,
        0,
        ".libra root must stay owner-only writable to protect the vault"
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn test_shared_vault_isolated() {
    for mode in ["group", "all"] {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();

        let output = run_libra_command(&["init", &format!("--shared={mode}")], &repo);
        assert_cli_success(&output, &format!("init --shared={mode} with vault"));

        let libra = repo.join(".libra");
        let vault = libra.join("vault.db");
        assert!(
            vault.exists(),
            "vault.db must exist with default --vault under --shared={mode}"
        );
        assert_eq!(
            mode_bits(&vault) & 0o777,
            0o600,
            "vault.db must be 0o600 under --shared={mode}"
        );
        assert_eq!(
            mode_bits(&libra) & 0o022,
            0,
            ".libra root must stay owner-only writable under --shared={mode}"
        );
    }
}

#[tokio::test]
async fn test_shared_writes_core_sharedrepository() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command(&["init", "--shared=group", "--vault", "false"], &repo);
    assert_cli_success(&output, "init --shared=group");

    let conn = open_repo_conn(&repo, false).await;
    assert_eq!(
        config_value(&conn, "core.sharedRepository")
            .await
            .as_deref(),
        Some("group"),
        "core.sharedRepository must be persisted for --shared=group"
    );
}

#[tokio::test]
async fn test_shared_value_canonicalization() {
    let cases = [
        ("true", "group"),
        ("world", "all"),
        ("everybody", "all"),
        ("umask", "umask"),
        ("false", "umask"),
        ("0660", "0660"),
    ];

    for (input, expected) in cases {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();

        let output = run_libra_command(
            &["init", &format!("--shared={input}"), "--vault", "false"],
            &repo,
        );
        assert_cli_success(&output, &format!("init --shared={input}"));

        let conn = open_repo_conn(&repo, false).await;
        assert_eq!(
            config_value(&conn, "core.sharedRepository")
                .await
                .as_deref(),
            Some(expected),
            "--shared={input} should canonicalize core.sharedRepository to {expected}"
        );
    }
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn test_shared_no_value_defaults_group() {
    let temp = tempdir().unwrap();

    // Bare `--shared` followed by a positional directory: with `require_equals`,
    // the directory is parsed as `repo_directory` (not swallowed as the value)
    // and the shared mode defaults to `group`.
    let output = run_libra_command(
        &["init", "--shared", "sub", "--vault", "false"],
        temp.path(),
    );
    assert_cli_success(&output, "init --shared sub");

    let repo = temp.path().join("sub");
    assert!(
        repo.join(".libra").exists(),
        "positional `sub` must be parsed as repo_directory, not the --shared value"
    );

    let conn = open_repo_conn(&repo, false).await;
    assert_eq!(
        config_value(&conn, "core.sharedRepository")
            .await
            .as_deref(),
        Some("group"),
        "bare --shared must default to group"
    );
    assert_eq!(
        mode_bits(&repo.join(".libra").join("objects")) & 0o020,
        0o020,
        "bare --shared must group-share content like --shared=group"
    );

    // The explicit `=` form for a sibling repo records the requested mode.
    let output2 = run_libra_command(
        &["init", "--shared=all", "--vault", "false", "sub2"],
        temp.path(),
    );
    assert_cli_success(&output2, "init --shared=all sub2");
    let conn2 = open_repo_conn(&temp.path().join("sub2"), false).await;
    assert_eq!(
        config_value(&conn2, "core.sharedRepository")
            .await
            .as_deref(),
        Some("all"),
        "--shared=all must record core.sharedRepository=all"
    );
}

#[test]
fn test_shared_config_get_roundtrip() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command(&["init", "--shared=group", "--vault", "false"], &repo);
    assert_cli_success(&output, "init --shared=group");

    let get = run_libra_command(&["config", "get", "core.sharedRepository"], &repo);
    assert_cli_success(&get, "config get core.sharedRepository");
    assert_eq!(
        String::from_utf8_lossy(&get.stdout).trim(),
        "group",
        "config get must read back the canonical shared mode"
    );
}

#[test]
fn test_shared_invalid_mode_exit_129() {
    let temp = tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    let output = run_libra_command(&["init", "--shared=invalid", "--vault", "false"], &repo);
    assert_eq!(
        output.status.code(),
        Some(129),
        "invalid --shared mode must exit 129"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid shared mode"),
        "expected invalid-mode message, got: {stderr}"
    );
    assert!(
        stderr.contains("LBR-CLI-002"),
        "expected CLI invalid-arguments code, got: {stderr}"
    );
    assert!(
        !repo.join(".libra").exists(),
        "no .libra layout may be created before the early validation failure"
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn test_shared_bare_repo() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(
        &[
            "init",
            "--bare",
            "--shared=group",
            "--vault",
            "false",
            "repo.git",
        ],
        temp.path(),
    );
    assert_cli_success(&output, "init --bare --shared=group");

    let bare = temp.path().join("repo.git");
    assert_eq!(
        mode_bits(&bare) & 0o022,
        0,
        "bare storage root must stay owner-only writable"
    );
    assert_eq!(
        mode_bits(&bare.join("objects")) & 0o020,
        0o020,
        "bare objects must be group-writable under --shared=group"
    );
}
