//! Tests config command read/write behaviors, scope handling, and edge cases.
//!
//! **Layer:** L1 — deterministic, no external dependencies.
//
use std::process::Command;

use libra::{CliErrorKind, command::config, exec_async};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Guard for temporarily setting an environment variable during a test and restoring it on drop.
///
/// # Safety
/// Modifying environment variables is process-global state. These tests are all annotated with
/// `#[serial]`, ensuring no concurrent mutation happens across tests.
struct EnvVarGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

#[tokio::test]
#[serial]
async fn test_cli_config_global_without_repo() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let global_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config_cli.db"));

    let result = exec_async(vec!["config", "--global", "user.name", "cli_global_user"]).await;
    assert!(result.is_ok());

    let read_result = exec_async(vec!["config", "--global", "--get", "user.name"]).await;
    assert!(read_result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_cli_config_list_global_without_repo() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let global_db_dir = tempdir().unwrap();
    let _scoped =
        ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config_cli_list.db"));

    let result = exec_async(vec!["config", "--list", "--global"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_cli_config_system_returns_error() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let global_db_dir = tempdir().unwrap();
    let _scoped =
        ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config_cli_sys.db"));

    // --system scope is removed and should always error
    let result = exec_async(vec!["config", "--system", "user.name", "cli_system_user"]).await;
    assert!(result.is_err(), "--system should be rejected");

    let result = exec_async(vec!["config", "--system", "--get", "user.name"]).await;
    assert!(result.is_err(), "--system --get should be rejected");

    let result = exec_async(vec!["config", "--list", "--system"]).await;
    assert!(result.is_err(), "--system --list should be rejected");
}

#[tokio::test]
#[serial]
async fn test_cli_config_local_requires_repo() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let result = exec_async(vec!["config", "--local", "--list"]).await;
    let err = result.unwrap_err();
    assert_eq!(err.kind(), CliErrorKind::Fatal);
    assert!(err.message().contains("not a libra repository"));
}

#[tokio::test]
#[serial]
async fn test_config_system_scope_is_rejected_as_command_usage_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(&["config", "--system", "list"], temp_path.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--system scope is not supported"),
        "stderr should describe the unsupported scope, got: {stderr}"
    );
    // config.md line 175: classifies as a CLI usage error (exit 2 fine /
    // 129 coarse). The previous `from_legacy_string` path collapsed this
    // to a generic failure (exit 128).
    assert_eq!(
        output.status.code(),
        Some(129),
        "--system rejection must classify as CLI usage (exit 129), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_import_global_from_git() {
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let global_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config_import.db"));

    let fake_home = tempdir().unwrap();
    let _home_guard = EnvVarGuard::set("HOME", fake_home.path().as_os_str());
    let _xdg_guard = EnvVarGuard::set(
        "XDG_CONFIG_HOME",
        fake_home.path().join(".config").as_os_str(),
    );

    let set_name = Command::new("git")
        .args(["config", "--global", "user.name", "Git Global Import User"])
        .output()
        .unwrap();
    assert!(set_name.status.success());

    let set_email = Command::new("git")
        .args([
            "config",
            "--global",
            "user.email",
            "git-global-import@example.com",
        ])
        .output()
        .unwrap();
    assert!(set_email.status.success());

    let result = exec_async(vec!["config", "--global", "import"]).await;
    assert!(result.is_ok());

    let imported_name = config::ScopedConfig::get(config::ConfigScope::Global, "user.name")
        .await
        .unwrap();
    let imported_email = config::ScopedConfig::get(config::ConfigScope::Global, "user.email")
        .await
        .unwrap();
    assert_eq!(
        imported_name.map(|e| e.value).as_deref(),
        Some("Git Global Import User")
    );
    assert_eq!(
        imported_email.map(|e| e.value).as_deref(),
        Some("git-global-import@example.com")
    );
}

#[tokio::test]
#[serial]
async fn test_config_import_local_from_git_repository() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    use libra::internal::config::ConfigKv;
    ConfigKv::unset_all("user.name").await.unwrap();
    ConfigKv::unset_all("user.email").await.unwrap();

    let git_init = Command::new("git").args(["init"]).output().unwrap();
    assert!(git_init.status.success());

    let set_name = Command::new("git")
        .args(["config", "user.name", "Git Local Import User"])
        .output()
        .unwrap();
    assert!(set_name.status.success());

    let set_email = Command::new("git")
        .args(["config", "user.email", "git-local-import@example.com"])
        .output()
        .unwrap();
    assert!(set_email.status.success());

    let result = exec_async(vec!["config", "import"]).await;
    assert!(result.is_ok());

    let imported_names: Vec<String> =
        config::ScopedConfig::get_all(config::ConfigScope::Local, "user.name")
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.value)
            .collect();
    let imported_emails: Vec<String> =
        config::ScopedConfig::get_all(config::ConfigScope::Local, "user.email")
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.value)
            .collect();
    assert!(imported_names.iter().any(|v| v == "Git Local Import User"));
    assert!(
        imported_emails
            .iter()
            .any(|v| v == "git-local-import@example.com")
    );
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: test is #[serial], so no concurrent env access/mutation across tests.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    fn unset(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: test is #[serial], so no concurrent env access/mutation across tests.
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test is #[serial], so no concurrent env access/mutation across tests.
        match &self.original {
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// Sets `LIBRA_CONFIG_GLOBAL_DB` to point at a temp file for isolation.
///
/// This prevents tests from touching real host paths like `~/.libra/config.db`.
struct ScopedConfigPathGuard {
    _global: EnvVarGuard,
}

impl ScopedConfigPathGuard {
    fn new(global_db_path: &std::path::Path) -> Self {
        let _global = EnvVarGuard::set("LIBRA_CONFIG_GLOBAL_DB", global_db_path.as_os_str());
        Self { _global }
    }
}

#[tokio::test]
#[serial]
async fn test_config_get_failed() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // --default with --add (no --get or --get-all) should error
    let result = exec_async(vec![
        "config",
        "--add",
        "-d",
        "erasernoob",
        "user.name",
        "value",
    ])
    .await;
    assert!(result.is_err());
}

#[tokio::test]
#[serial]
async fn test_config_get_all() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let result = exec_async(vec!["config", "--add", "user.name", "erasernoob"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec!["config", "--get", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_get_all_with_default() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());
    let global_db_dir = tempdir().unwrap();
    let _scoped =
        ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config_get_all.db"));

    let result = exec_async(vec!["config", "--get-all", "-d", "erasernoob", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_get() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let result = exec_async(vec!["config", "--add", "user.name", "erasernoob"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec!["config", "--get", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_get_with_default() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let result = exec_async(vec!["config", "--get", "-d", "erasernoob", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_list() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let result = exec_async(vec!["config", "--add", "user.name", "erasernoob"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec![
        "config",
        "--add",
        "user.email",
        "erasernoob@example.com",
    ])
    .await;
    assert!(result.is_ok());

    // List configs
    let result = exec_async(vec!["config", "--list"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_list_name_only() {
    let temp_path = tempdir().unwrap();
    // start a new libra repository in a temporary directory
    test::setup_with_new_libra_in(temp_path.path()).await;

    // set the current working directory to the temporary path
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Add the config first
    let result = exec_async(vec!["config", "--add", "user.name", "erasernoob"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec![
        "config",
        "--add",
        "user.email",
        "erasernoob@example.com",
    ])
    .await;
    assert!(result.is_ok());

    // List configs with name_only via subcommand
    let result = exec_async(vec!["config", "list", "--name-only"]).await;
    assert!(result.is_ok());
}

// New tests for scope functionality
#[tokio::test]
#[serial]
async fn test_config_scope_local_default() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Test that no scope specified defaults to local
    let result = exec_async(vec!["config", "user.name", "test_user_local_default"]).await;
    assert!(result.is_ok());

    // Verify the value was written to local scope by reading it back
    let result = exec_async(vec!["config", "--get", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_scope_global() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Isolate global DB paths to temp files (no host pollution).
    let global_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config.db"));

    // Set a value in global scope
    let result = exec_async(vec![
        "config",
        "--global",
        "user.email",
        "global_user@example.com",
    ])
    .await;
    assert!(result.is_ok());

    // Verify the value was written to global scope by reading it back
    let result = exec_async(vec!["config", "--global", "--get", "user.email"]).await;
    assert!(result.is_ok());

    // Verify that the global value is NOT accessible from local scope
    let result = exec_async(vec![
        "config",
        "--local",
        "--get",
        "-d",
        "not_found",
        "user.email",
    ])
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_scope_system_errors() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // --system scope is removed and should always error
    let result = exec_async(vec!["config", "--system", "user.name", "system_user"]).await;
    assert!(result.is_err(), "--system should be rejected");
    let err = result.unwrap_err();
    assert!(
        err.message().contains("--system scope is not supported"),
        "unexpected error: {}",
        err.message()
    );
}

#[tokio::test]
#[serial]
async fn test_config_scope_explicit_local() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Set a value explicitly in local scope
    let result = exec_async(vec![
        "config",
        "--local",
        "user.name",
        "explicit_local_user",
    ])
    .await;
    assert!(result.is_ok());

    // Verify the value was written to local scope by reading it back
    let result = exec_async(vec!["config", "--local", "--get", "user.name"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_scope_isolation() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Isolate global DB paths to temp files (no host pollution).
    let global_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(&global_db_dir.path().join("global_config.db"));

    // Set the same key with different values in different scopes
    let result = exec_async(vec!["config", "--local", "test.isolation", "local_value"]).await;
    assert!(result.is_ok());

    let result = exec_async(vec!["config", "--global", "test.isolation", "global_value"]).await;
    assert!(result.is_ok());

    // Verify that each scope returns its own value
    println!("Reading from local scope:");
    let result = exec_async(vec!["config", "--local", "--get", "test.isolation"]).await;
    assert!(result.is_ok());

    println!("Reading from global scope:");
    let result = exec_async(vec!["config", "--global", "--get", "test.isolation"]).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn test_config_get_reveal_decrypt_failure_returns_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::vault::lazy_init_vault_for_scope("local")
        .await
        .unwrap();
    libra::internal::config::ConfigKv::set("vault.env.TEST_SECRET", "not-valid-hex", true)
        .await
        .unwrap();

    let result = exec_async(vec!["config", "get", "--reveal", "vault.env.TEST_SECRET"]).await;
    let err = result.expect_err("decrypt failure should surface as an error");
    assert_eq!(err.kind(), CliErrorKind::Fatal);
    assert_eq!(err.exit_code(), 128);
    assert!(
        err.message()
            .contains("failed to decrypt value for key 'vault.env.TEST_SECRET'")
    );
}

#[tokio::test]
#[serial]
async fn test_config_get_cascaded_global_read_failure_returns_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let bad_global_db = temp_path.path().join("bad-global.db");
    std::fs::write(&bad_global_db, "definitely-not-a-sqlite-database").unwrap();
    let _scoped = ScopedConfigPathGuard::new(&bad_global_db);

    let result = exec_async(vec!["config", "get", "user.missing"]).await;
    let err = result.expect_err("broken cascaded scope should not be ignored");
    assert_eq!(err.kind(), CliErrorKind::Fatal);
    assert_eq!(err.exit_code(), 128);
    assert!(err.message().contains("failed to read global config"));
}

#[tokio::test]
#[serial]
async fn test_config_add_rejects_implicit_encryption_mixed_with_existing_plaintext() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let result = exec_async(vec![
        "config",
        "set",
        "--plaintext",
        "custom.token",
        "plaintext-token",
    ])
    .await;
    assert!(result.is_ok());

    let result = exec_async(vec![
        "config",
        "set",
        "--add",
        "custom.token",
        "second-token",
    ])
    .await;
    let err = result.expect_err("implicit auto-encryption should not mix with plaintext values");
    assert!(
        err.message()
            .contains("cannot mix encrypted and plaintext values for the same key"),
        "unexpected error: {}",
        err.message()
    );

    let entries = config::ScopedConfig::get_all(config::ConfigScope::Local, "custom.token")
        .await
        .unwrap();
    assert_eq!(entries.len(), 1, "mixed-state insert should be rejected");
    assert!(
        !entries[0].encrypted,
        "original plaintext entry should remain"
    );
    assert_eq!(entries[0].value, "plaintext-token");
}

#[tokio::test]
#[serial]
async fn test_config_set_encrypt_plaintext_mutex_is_command_usage_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &[
            "config",
            "set",
            "--encrypt",
            "--plaintext",
            "custom.token",
            "value",
        ],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--encrypt and --plaintext are mutually exclusive"),
        "stderr should describe the mutex violation, got: {stderr}"
    );
    // config.md line 77: classified as a usage error (exit 2 fine / 129 coarse).
    assert_eq!(
        output.status.code(),
        Some(129),
        "mutex flag error must classify as CLI usage (exit 129), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_stdin_with_positional_value_is_command_usage_error() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &["config", "set", "--stdin", "custom.token", "value"],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot use both value argument and --stdin"),
        "stderr should describe the --stdin vs positional mutex, got: {stderr}"
    );
    // config.md line 144: usage error (exit 2 fine / 129 coarse).
    assert_eq!(
        output.status.code(),
        Some(129),
        "--stdin + positional must classify as CLI usage (exit 129), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_plaintext_on_vault_internal_key_is_failure() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &[
            "config",
            "set",
            "--plaintext",
            "vault.env.API_KEY",
            "secret-value",
        ],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--plaintext cannot be used with vault internal/secret keys"),
        "stderr should describe the secret-key plaintext reject, got: {stderr}"
    );
    // config.md line 77: validation reject (exit 1 fine / 128 coarse) — must
    // classify as a runtime Failure (exit 128) rather than the previous
    // legacy-string fallthrough that produced the same number but with the
    // internal-invariant stable code.
    assert_eq!(
        output.status.code(),
        Some(128),
        "vault internal key plaintext reject must classify as Failure (exit 128), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_read_failure_does_not_silently_skip_existing_state_check() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());
    // Prevent any interactive prompts from blocking the test.
    let _test_env = EnvVarGuard::set("LIBRA_TEST", std::ffi::OsStr::new("1"));

    let bad_global_dir = tempdir().unwrap();
    let bad_global_db = bad_global_dir.path().join("bad-global.db");
    std::fs::write(&bad_global_db, "definitely-not-a-sqlite-database").unwrap();
    let _scoped = ScopedConfigPathGuard::new(&bad_global_db);

    let fake_home = tempdir().unwrap();
    let _home_guard = EnvVarGuard::set("HOME", fake_home.path().as_os_str());
    let _userprofile_guard = EnvVarGuard::set("USERPROFILE", fake_home.path().as_os_str());

    let result = exec_async(vec![
        "config",
        "set",
        "--global",
        "vault.env.TEST_SECRET",
        "super-secret",
    ])
    .await;
    let err = result.expect_err("broken config read should surface before write/lazy-init");
    assert_eq!(err.kind(), CliErrorKind::Fatal);
    assert_eq!(err.exit_code(), 128);
    assert!(
        err.message()
            .contains("failed to read global config while checking existing values"),
        "unexpected error: {}",
        err.message()
    );

    assert!(
        !fake_home
            .path()
            .join(".libra")
            .join("vault-unseal-key")
            .exists(),
        "failed existing-state lookup should not trigger global vault lazy init"
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_missing_value_uses_protected_input_when_existing_key_is_encrypted() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());
    // Prevent rpassword::read_password() from blocking on stdin.
    let _test_env = EnvVarGuard::set("LIBRA_TEST", std::ffi::OsStr::new("1"));

    let result = exec_async(vec![
        "config",
        "set",
        "--encrypt",
        "custom.value",
        "encrypted-value",
    ])
    .await;
    assert!(result.is_ok());

    let result = exec_async(vec!["config", "set", "custom.value"]).await;
    let err = result.expect_err("existing encrypted state should require protected input");
    assert_eq!(err.exit_code(), 2);
    assert!(
        err.message()
            .contains("missing value for protected key 'custom.value'"),
        "unexpected error: {}",
        err.message()
    );
}

#[tokio::test]
#[serial]
async fn test_config_list_defaults_to_local_scope_without_global_entries() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::config::ConfigKv::set("user.name", "local-user", false)
        .await
        .unwrap();

    let child_home = temp_path.path().join(".libra-test-home");
    let child_global_dir = child_home.join(".libra");
    std::fs::create_dir_all(&child_global_dir).unwrap();
    let child_global_db = child_global_dir.join("config.db");
    let global_conn =
        libra::internal::db::create_database(child_global_db.to_string_lossy().as_ref())
            .await
            .unwrap();
    libra::internal::config::ConfigKv::set_with_conn(&global_conn, "core.editor", "vim", false)
        .await
        .unwrap();

    let output = run_libra_command(&["config", "list"], temp_path.path());
    assert!(
        output.status.success(),
        "config list should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("user.name=local-user"),
        "local entry should be listed, stdout: {stdout}"
    );
    assert!(
        !stdout.contains("core.editor"),
        "default list should not include global entries, stdout: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_list_ssh_keys_outputs_configured_public_keys() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::config::ConfigKv::set(
        "vault.ssh.origin.pubkey",
        "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC origin-key",
        false,
    )
    .await
    .unwrap();
    libra::internal::config::ConfigKv::set(
        "vault.ssh.upstream.pubkey",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA upstream-key",
        false,
    )
    .await
    .unwrap();
    libra::internal::config::ConfigKv::set("vault.ssh.origin.privkey", "ciphertext", true)
        .await
        .unwrap();

    let output = run_libra_command(&["config", "list", "--ssh-keys"], temp_path.path());
    assert!(
        output.status.success(),
        "config list --ssh-keys should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("SSH keys:"), "stdout: {stdout}");
    assert!(stdout.contains("origin"), "stdout: {stdout}");
    assert!(stdout.contains("upstream"), "stdout: {stdout}");
    assert!(
        stdout.contains("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC origin-key"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("ciphertext"),
        "private key entries must not be listed, stdout: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_list_gpg_keys_outputs_configured_key_namespaces() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::config::ConfigKv::set(
        "vault.gpg.pubkey",
        "-----BEGIN PGP PUBLIC KEY BLOCK-----\nSIGNING\n-----END PGP PUBLIC KEY BLOCK-----",
        false,
    )
    .await
    .unwrap();
    libra::internal::config::ConfigKv::set(
        "vault.gpg.encrypt.pubkey",
        "-----BEGIN PGP PUBLIC KEY BLOCK-----\nENCRYPT\n-----END PGP PUBLIC KEY BLOCK-----",
        false,
    )
    .await
    .unwrap();
    libra::internal::config::ConfigKv::set("vault.signing", "true", false)
        .await
        .unwrap();

    let output = run_libra_command(&["config", "list", "--gpg-keys"], temp_path.path());
    assert!(
        output.status.success(),
        "config list --gpg-keys should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("GPG keys:"), "stdout: {stdout}");
    assert!(stdout.contains("signing"), "stdout: {stdout}");
    assert!(stdout.contains("encrypt"), "stdout: {stdout}");
    assert!(
        stdout.contains("vault.gpg.pubkey"),
        "signing pubkey key should be listed, stdout: {stdout}"
    );
    assert!(
        stdout.contains("vault.gpg.encrypt.pubkey"),
        "encrypt pubkey key should be listed, stdout: {stdout}"
    );
    assert!(
        stdout.contains("vault.signing = true"),
        "signing-enabled hint should be listed, stdout: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_ssh_key_replaces_vault_generate_ssh_key_flow() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let remote = run_libra_command(
        &["remote", "add", "origin", "git@github.com:example/repo.git"],
        temp_path.path(),
    );
    assert_cli_success(&remote, "remote add origin");

    let output = run_libra_command(
        &["config", "generate-ssh-key", "--remote", "origin"],
        temp_path.path(),
    );
    assert_cli_success(&output, "config generate-ssh-key --remote origin");

    let pubkey = libra::internal::config::ConfigKv::get("vault.ssh.origin.pubkey")
        .await
        .unwrap()
        .expect("config generate-ssh-key should store a public key");
    assert!(
        pubkey.value.starts_with("ssh-rsa "),
        "expected RSA SSH public key, got: {}",
        pubkey.value
    );

    let privkey = libra::internal::config::ConfigKv::get("vault.ssh.origin.privkey")
        .await
        .unwrap()
        .expect("config generate-ssh-key should store an encrypted private key");
    assert!(privkey.encrypted, "private key must stay vault-encrypted");
    assert!(
        !privkey.value.contains("PRIVATE KEY"),
        "private key must not be stored as plaintext"
    );

    let get_output = run_libra_command(
        &["config", "get", "vault.ssh.origin.pubkey"],
        temp_path.path(),
    );
    assert_cli_success(&get_output, "config get vault.ssh.origin.pubkey");
    let stdout = String::from_utf8_lossy(&get_output.stdout);
    assert!(stdout.contains("ssh-rsa "), "stdout: {stdout}");
}

#[tokio::test]
#[serial]
async fn test_config_generate_global_ssh_key_is_rejected_without_local_side_effects() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let remote = run_libra_command(
        &["remote", "add", "origin", "git@github.com:example/repo.git"],
        temp_path.path(),
    );
    assert_cli_success(&remote, "remote add origin");

    libra::internal::config::ConfigKv::unset_all("vault.ssh.origin.pubkey")
        .await
        .unwrap();
    libra::internal::config::ConfigKv::unset_all("vault.ssh.origin.privkey")
        .await
        .unwrap();

    let output = run_libra_command(
        &[
            "config",
            "--global",
            "generate-ssh-key",
            "--remote",
            "origin",
        ],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("generate-ssh-key only supports local scope"),
        "stderr should explain unsupported global SSH key generation, got: {stderr}"
    );
    assert!(
        stderr.contains("run without --global"),
        "stderr should tell users how to run the supported form, got: {stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "global generate-ssh-key should be a command usage error, got status: {:?}, stderr: {stderr}",
        output.status,
    );

    assert!(
        libra::internal::config::ConfigKv::get("vault.ssh.origin.pubkey")
            .await
            .unwrap()
            .is_none(),
        "--global generate-ssh-key must not write a local public key"
    );
    assert!(
        libra::internal::config::ConfigKv::get("vault.ssh.origin.privkey")
            .await
            .unwrap()
            .is_none(),
        "--global generate-ssh-key must not write a local private key"
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_ssh_key_rejects_invalid_remote_name_as_command_usage() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &["config", "generate-ssh-key", "--remote", "bad.name"],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid remote name 'bad.name'"),
        "stderr should describe the validation failure, got: {stderr}"
    );
    // CLI usage errors map to exit code 129 in coarse mode (Cli category →
    // CliExitCode::Usage). The previous implementation collapsed both the
    // invalid-name and missing-remote branches into `failure` (exit 128),
    // which is the wrong category for a user-supplied bad argument.
    assert_eq!(
        output.status.code(),
        Some(129),
        "invalid remote name must classify as a CLI usage error (exit 129), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_ssh_key_rejects_unknown_remote_with_invalid_target_code() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &["config", "generate-ssh-key", "--remote", "no-such-remote"],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("remote 'no-such-remote' not found"),
        "stderr should describe the missing remote, got: {stderr}"
    );
    // Missing remote is a Fatal failure (exit 128 in coarse mode) — the
    // user-supplied name passed validation but the resource does not exist
    // at the time of execution.
    assert_eq!(
        output.status.code(),
        Some(128),
        "unknown remote must classify as a fatal failure (exit 128), got status: {:?}, stderr: {stderr}",
        output.status,
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_gpg_key_replaces_vault_generate_gpg_key_flow() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let output = run_libra_command(
        &[
            "config",
            "generate-gpg-key",
            "--name",
            "Config User",
            "--email",
            "config@example.com",
        ],
        temp_path.path(),
    );
    assert_cli_success(&output, "config generate-gpg-key");

    let pubkey = libra::internal::config::ConfigKv::get("vault.gpg.pubkey")
        .await
        .unwrap()
        .expect("config generate-gpg-key should store the signing public key");
    assert!(
        pubkey.value.contains("BEGIN PGP PUBLIC KEY BLOCK"),
        "expected armored PGP public key, got: {}",
        pubkey.value
    );

    let generated_stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        generated_stdout.contains("Config User <config@example.com>"),
        "expected configured user ID in command output, stdout: {generated_stdout}"
    );

    let signing = libra::internal::config::ConfigKv::get("vault.signing")
        .await
        .unwrap()
        .expect("signing key generation should enable vault signing");
    assert_eq!(signing.value, "true");

    let get_output = run_libra_command(&["config", "get", "vault.gpg.pubkey"], temp_path.path());
    assert_cli_success(&get_output, "config get vault.gpg.pubkey");
    let stdout = String::from_utf8_lossy(&get_output.stdout);
    assert!(
        stdout.contains("BEGIN PGP PUBLIC KEY BLOCK"),
        "stdout: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_global_gpg_key_is_rejected_without_local_side_effects() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    libra::internal::config::ConfigKv::unset_all("vault.gpg.pubkey")
        .await
        .unwrap();
    libra::internal::config::ConfigKv::unset_all("vault.signing")
        .await
        .unwrap();

    let output = run_libra_command(
        &[
            "config",
            "--global",
            "generate-gpg-key",
            "--name",
            "Global User",
            "--email",
            "global@example.com",
        ],
        temp_path.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("generate-gpg-key only supports local scope"),
        "stderr should explain unsupported global GPG key generation, got: {stderr}"
    );
    assert!(
        stderr.contains("run without --global"),
        "stderr should tell users how to run the supported form, got: {stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "global generate-gpg-key should be a command usage error, got status: {:?}, stderr: {stderr}",
        output.status,
    );

    assert!(
        libra::internal::config::ConfigKv::get("vault.gpg.pubkey")
            .await
            .unwrap()
            .is_none(),
        "--global generate-gpg-key must not write a local GPG public key"
    );
    assert!(
        libra::internal::config::ConfigKv::get("vault.signing")
            .await
            .unwrap()
            .is_none(),
        "--global generate-gpg-key must not enable local vault signing"
    );
}

#[tokio::test]
#[serial]
async fn test_config_generate_gpg_key_rejects_invalid_usage() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let output = run_libra_command(
        &["config", "generate-gpg-key", "--usage", "archive"],
        temp_path.path(),
    );
    assert!(
        !output.status.success(),
        "generate-gpg-key should reject unsupported usage"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid value 'archive'"),
        "stderr should explain invalid usage, stderr: {stderr}"
    );
    assert!(
        stderr.contains("signing") && stderr.contains("encrypt"),
        "stderr should list supported usages, stderr: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_scope_path_logic() {
    // Test the path logic for different scopes without executing config operations

    // Local scope should return None (uses repository database)
    assert_eq!(config::ConfigScope::Local.get_config_path(), None);

    // Global scope should return a path in the home directory (if available)
    let global_path = config::ConfigScope::Global.get_config_path();
    if dirs::home_dir().is_some() {
        assert!(global_path.is_some());
        let path = global_path.unwrap();
        assert!(path.to_string_lossy().contains(".libra"));
        assert!(path.to_string_lossy().ends_with("config.db"));
    } else {
        // In environments without home directory, should return None
        assert_eq!(global_path, None);
    }
}

#[tokio::test]
#[serial]
async fn test_config_cross_platform_paths() {
    // Test that all scopes return appropriate paths for the current platform

    // Local scope should always return None (uses repository database)
    assert_eq!(config::ConfigScope::Local.get_config_path(), None);

    // Global scope behavior (should work on all platforms with home directory)
    let global_path = config::ConfigScope::Global.get_config_path();
    if dirs::home_dir().is_some() {
        assert!(global_path.is_some());
        let path = global_path.unwrap();
        assert!(path.to_string_lossy().contains(".libra"));
        assert!(path.to_string_lossy().ends_with("config.db"));

        // Verify the path uses the correct separator for the platform
        #[cfg(windows)]
        {
            // On Windows, paths should use backslashes or be properly normalized
            let path_str = path.to_string_lossy();
            assert!(path_str.contains("libra") && path_str.contains("config.db"));
        }
        #[cfg(unix)]
        {
            // On Unix, paths should use forward slashes
            assert!(path.to_string_lossy().contains("/"));
        }
    }
}

/// Regression: a corrupted/incompatible `~/.libra/config.db` must not block
/// identity resolution.
///
/// Reproduced from a real 0.17.500 user report: `libra clone` aborted with
/// "fatal: vault initialization failed: failed to open config database
/// '/home/eli/.libra/config.db'" because the global config DB existed but
/// could not be opened (the only fix path was to delete the file). After
/// v0.17.515 `resolve_user_identity_sources` downgrades that failure to a
/// warning and returns `Ok` with `config_*` set to `None`, letting init
/// fall back to env vars / "Libra User" defaults.
#[tokio::test]
#[serial]
async fn resolve_user_identity_sources_tolerates_corrupt_global_db() {
    use libra::internal::config::{LocalIdentityTarget, resolve_user_identity_sources};

    let temp_dir = tempdir().unwrap();
    let global_db_path = temp_dir.path().join("corrupt_config.db");
    // A non-SQLite payload: opening this file as a sea-orm SQLite connection
    // (or running the schema-compat check on it) is guaranteed to fail.
    std::fs::write(&global_db_path, b"this is not a sqlite database").unwrap();

    let _global = EnvVarGuard::set("LIBRA_CONFIG_GLOBAL_DB", global_db_path.as_os_str());

    // Ensure env-var fallbacks are empty so we can attribute the result to
    // config-read tolerance, not env shadowing.
    let _git_committer_name = EnvVarGuard::set("GIT_COMMITTER_NAME", std::ffi::OsStr::new(""));
    let _git_committer_email = EnvVarGuard::set("GIT_COMMITTER_EMAIL", std::ffi::OsStr::new(""));
    let _git_author_name = EnvVarGuard::set("GIT_AUTHOR_NAME", std::ffi::OsStr::new(""));
    let _git_author_email = EnvVarGuard::set("GIT_AUTHOR_EMAIL", std::ffi::OsStr::new(""));
    let _email = EnvVarGuard::set("EMAIL", std::ffi::OsStr::new(""));
    let _libra_committer_name = EnvVarGuard::set("LIBRA_COMMITTER_NAME", std::ffi::OsStr::new(""));
    let _libra_committer_email =
        EnvVarGuard::set("LIBRA_COMMITTER_EMAIL", std::ffi::OsStr::new(""));

    let sources = resolve_user_identity_sources(LocalIdentityTarget::None)
        .await
        .expect("identity resolution must not propagate global DB read failures");

    assert!(
        sources.config_name.is_none(),
        "expected config_name to be None when global DB is unreadable, got {:?}",
        sources.config_name
    );
    assert!(
        sources.config_email.is_none(),
        "expected config_email to be None when global DB is unreadable, got {:?}",
        sources.config_email
    );
}

/// `resolve_env_for_target` is the shared secret resolver used by provider,
/// D1, R2, and tool credential paths. Per the 12-Factor / docs/improvement/
/// config.md spec, the priority is **process env > local vault > global vault**
/// so a per-process override like `GEMINI_API_KEY=B libra push` always wins.
/// Local vault is the fallback when env is unset.
#[tokio::test]
#[serial]
async fn resolve_env_for_target_process_env_overrides_local_vault() {
    use libra::internal::config::{ConfigKv, LocalIdentityTarget, resolve_env_for_target};

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _cwd = test::ChangeDirGuard::new(temp_path.path());

    let _env = EnvVarGuard::set(
        "LIBRA_RESOLVE_ENV_PRIORITY_KEY",
        std::ffi::OsStr::new("env-value"),
    );
    let _global = EnvVarGuard::set(
        "LIBRA_CONFIG_GLOBAL_DB",
        std::ffi::OsStr::new("/nonexistent/resolve-env-priority-local.db"),
    );

    ConfigKv::set(
        "vault.env.LIBRA_RESOLVE_ENV_PRIORITY_KEY",
        "vault-value",
        false,
    )
    .await
    .unwrap();

    // env wins; per-process override is sacred (12-Factor).
    let value = resolve_env_for_target(
        "LIBRA_RESOLVE_ENV_PRIORITY_KEY",
        LocalIdentityTarget::CurrentRepo,
    )
    .await
    .unwrap();
    assert_eq!(value.as_deref(), Some("env-value"));

    // …and when the env is unset, the local vault fallback is used.
    drop(_env);
    let value = resolve_env_for_target(
        "LIBRA_RESOLVE_ENV_PRIORITY_KEY",
        LocalIdentityTarget::CurrentRepo,
    )
    .await
    .unwrap();
    assert_eq!(value.as_deref(), Some("vault-value"));
}

/// Same priority chain in the `LocalIdentityTarget::None` mode used by
/// commands that can run outside a Libra worktree (provider/bootstrap path).
/// process env > global vault.
#[tokio::test]
#[serial]
async fn resolve_env_for_target_process_env_overrides_global_vault() {
    use libra::internal::{
        config::{ConfigKv, LocalIdentityTarget, resolve_env_for_target},
        db,
    };

    let _guard = EnvVarGuard::set(
        "LIBRA_RESOLVE_ENV_GLOBAL_PRIORITY_KEY",
        std::ffi::OsStr::new("env-value"),
    );
    let global_dir = tempdir().unwrap();
    let global_db_path = global_dir.path().join("global-config.db");
    let _global = EnvVarGuard::set("LIBRA_CONFIG_GLOBAL_DB", global_db_path.as_os_str());
    let global_conn = db::create_database(global_db_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    ConfigKv::set_with_conn(
        &global_conn,
        "vault.env.LIBRA_RESOLVE_ENV_GLOBAL_PRIORITY_KEY",
        "global-vault-value",
        false,
    )
    .await
    .unwrap();

    // env wins.
    let value = resolve_env_for_target(
        "LIBRA_RESOLVE_ENV_GLOBAL_PRIORITY_KEY",
        LocalIdentityTarget::None,
    )
    .await
    .unwrap();
    assert_eq!(value.as_deref(), Some("env-value"));

    // …and global vault is the fallback when env is unset.
    drop(_guard);
    let value = resolve_env_for_target(
        "LIBRA_RESOLVE_ENV_GLOBAL_PRIORITY_KEY",
        LocalIdentityTarget::None,
    )
    .await
    .unwrap();
    assert_eq!(value.as_deref(), Some("global-vault-value"));
}

/// Process env remains the final fallback when neither local nor global Vault
/// supplies the key.
#[tokio::test]
#[serial]
async fn resolve_env_sync_falls_back_to_process_env_when_vault_missing() {
    use libra::internal::config::resolve_env_sync;

    let _guard = EnvVarGuard::set(
        "LIBRA_RESOLVE_ENV_SYNC_TEST_KEY",
        std::ffi::OsStr::new("env-fallback"),
    );
    let _global = EnvVarGuard::set(
        "LIBRA_CONFIG_GLOBAL_DB",
        std::ffi::OsStr::new("/nonexistent/resolve-env-sync-fallback-path.db"),
    );

    let value = resolve_env_sync("LIBRA_RESOLVE_ENV_SYNC_TEST_KEY").unwrap();
    assert_eq!(value.as_deref(), Some("env-fallback"));
}

/// Absence path: when no process env, no repo, and no global DB layer carries
/// the key, the wrapper returns `Ok(None)` (not an error). A schema-mismatch
/// on the global DB is treated as missing-value here (the underlying
/// `resolve_env_for_target` already downgrades that to `tracing::warn!`),
/// matching the v0.17.515 / v0.17.534 fallback contract.
#[tokio::test]
#[serial]
async fn resolve_env_sync_returns_none_when_no_layer_supplies_value() {
    use libra::internal::config::resolve_env_sync;

    let _guard = EnvVarGuard::unset("LIBRA_RESOLVE_ENV_SYNC_ABSENT_KEY");
    let _global = EnvVarGuard::set(
        "LIBRA_CONFIG_GLOBAL_DB",
        std::ffi::OsStr::new("/nonexistent/resolve-env-sync-absent-path.db"),
    );
    let temp_dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_dir.path());

    let value = resolve_env_sync("LIBRA_RESOLVE_ENV_SYNC_ABSENT_KEY").unwrap();
    assert!(
        value.is_none(),
        "expected None for an unset key, got {value:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Wave 2 — script-safe output flags (--null / --show-origin / --show-scope /
// --name-only). Process-level assertions on exact bytes and exit codes.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_config_list_null_uses_git_record_format() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    run_libra_command(&["config", "set", "user.name", "Ada"], temp_path.path());
    let output = run_libra_command(&["config", "--list", "--null"], temp_path.path());

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Git-style record: key\nvalue\0 (NOT key=value\0).
    assert!(
        stdout.contains("user.name\nAda\0"),
        "expected Git null record `user.name\\nAda\\0`, got: {stdout:?}"
    );
    assert!(
        !stdout.contains("user.name=Ada\0"),
        "null mode must not emit `key=value\\0`, got: {stdout:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_get_null_emits_nul_terminated_value() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    run_libra_command(&["config", "set", "user.name", "Ada"], temp_path.path());
    let output = run_libra_command(
        &["config", "--get", "user.name", "--null"],
        temp_path.path(),
    );

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"Ada\0", "get --null must emit `Ada\\0`");
}

#[tokio::test]
#[serial]
async fn test_config_get_all_null_one_record_per_value() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    run_libra_command(
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/a:refs/remotes/origin/a",
        ],
        temp_path.path(),
    );
    run_libra_command(
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/b:refs/remotes/origin/b",
        ],
        temp_path.path(),
    );

    let output = run_libra_command(
        &["config", "--get-all", "remote.origin.fetch", "--null"],
        temp_path.path(),
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stdout,
        b"+refs/heads/a:refs/remotes/origin/a\0+refs/heads/b:refs/remotes/origin/b\0".to_vec(),
        "get-all --null must emit one NUL-terminated record per value"
    );
}

#[tokio::test]
#[serial]
async fn test_config_get_regexp_name_only_keys_without_values() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    run_libra_command(
        &["config", "set", "remote.origin.url", "ssh://example/x.git"],
        temp_path.path(),
    );
    let output = run_libra_command(
        &["config", "--get-regexp", "^remote\\.", "--name-only"],
        temp_path.path(),
    );
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("remote.origin.url\n"),
        "name-only must list the key, got: {stdout:?}"
    );
    assert!(
        !stdout.contains("ssh://example") && !stdout.contains('='),
        "name-only must not emit the value or `=`, got: {stdout:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_get_regexp_text_is_space_separated() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    run_libra_command(
        &["config", "set", "remote.origin.url", "ssh://example/x.git"],
        temp_path.path(),
    );
    let output = run_libra_command(&["config", "--get-regexp", "^remote\\."], temp_path.path());
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Git-style `key value` (space), NOT the historical `key = value`.
    assert!(
        stdout.contains("remote.origin.url ssh://example/x.git\n"),
        "get-regexp text must be space-separated `key value`, got: {stdout:?}"
    );
    assert!(
        !stdout.contains("remote.origin.url = "),
        "get-regexp must not use the legacy `key = value` format, got: {stdout:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_single_get_name_only_is_rejected() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    run_libra_command(&["config", "set", "user.name", "Ada"], temp_path.path());

    for args in [
        vec!["config", "--get", "user.name", "--name-only"],
        vec!["config", "get", "user.name", "--name-only"],
    ] {
        let output = run_libra_command(&args, temp_path.path());
        assert_eq!(
            output.status.code(),
            Some(129),
            "single-key get --name-only must exit 129, args: {args:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("name-only is only supported for list and --get-regexp"),
            "stderr should explain the restriction, got: {stderr}"
        );
    }
}

#[tokio::test]
#[serial]
async fn test_config_list_show_origin_and_scope() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    run_libra_command(&["config", "set", "user.name", "Ada"], temp_path.path());
    let output = run_libra_command(
        &["config", "--list", "--show-origin", "--show-scope"],
        temp_path.path(),
    );
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // scope<TAB>file:<path><TAB>key=value — scope label AND a file: origin.
    assert!(
        stdout.contains("local\tfile:") && stdout.contains("\tuser.name=Ada\n"),
        "show-origin+show-scope must emit `local<TAB>file:<path><TAB>user.name=Ada`, got: {stdout:?}"
    );
    // Origin must be a file path, not the scope label masquerading as origin.
    assert!(
        stdout.contains(".libra"),
        "origin path should point at a `.libra` SQLite DB, got: {stdout:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_json_null_is_rejected() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let output = run_libra_command(&["--json", "config", "--list", "--null"], temp_path.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "--json --null must exit 129, got: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--null is not compatible with JSON output"),
        "stderr should explain the JSON/null conflict, got: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_json_show_origin_scope_adds_fields_without_breaking_origin() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    run_libra_command(&["config", "set", "user.name", "Ada"], temp_path.path());
    let output = run_libra_command(
        &[
            "--json",
            "config",
            "--list",
            "--show-origin",
            "--show-scope",
        ],
        temp_path.path(),
    );
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let entries = json["data"]["entries"].as_array().expect("entries array");
    let entry = entries
        .iter()
        .find(|e| e["key"] == "user.name")
        .expect("user.name entry present");
    // Existing `origin` field keeps its scope-label meaning.
    assert_eq!(
        entry["origin"], "local",
        "origin must remain the scope label"
    );
    // New precise fields.
    assert_eq!(entry["scope"], "local");
    assert_eq!(entry["origin_type"], "file");
    assert!(
        entry["origin_path"]
            .as_str()
            .unwrap_or("")
            .contains(".libra"),
        "origin_path must be the backing SQLite DB path, got: {entry:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_stdin_null_is_rejected_before_stdin_consumed() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let output = run_libra_command_with_stdin(
        &["config", "set", "custom.value", "--stdin", "--null"],
        temp_path.path(),
        "super-secret",
    );
    assert_eq!(
        output.status.code(),
        Some(129),
        "--stdin --null must exit 129, got: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("--null controls output delimiters and cannot be used to parse stdin values"),
        "stderr should explain the stdin/null conflict, got: {stderr}"
    );
    // The value must not have been stored.
    let got = run_libra_command(&["config", "--get", "custom.value"], temp_path.path());
    assert_ne!(
        got.status.code(),
        Some(0),
        "custom.value must not be stored"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Wave 3 — multi-value git semantics, value filtering, and key validation
// ─────────────────────────────────────────────────────────────────────────────

/// Add two refspec values to `remote.origin.fetch` in `cwd`.
#[cfg(test)]
fn seed_two_fetch_values(cwd: &std::path::Path) {
    run_libra_command(
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/a:refs/remotes/origin/a",
        ],
        cwd,
    );
    run_libra_command(
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/b:refs/remotes/origin/b",
        ],
        cwd,
    );
}

#[tokio::test]
#[serial]
async fn test_config_append_and_add_are_equivalent_and_do_not_replace() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    run_libra_command(
        &["config", "set", "--append", "remote.origin.fetch", "v1"],
        temp_path.path(),
    );
    run_libra_command(
        &["config", "--add", "remote.origin.fetch", "v2"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &["config", "--get-all", "remote.origin.fetch"],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("v1\n") && stdout.contains("v2\n"),
        "both values kept, got: {stdout:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_default_set_on_multivalue_is_ambiguous_exit_5() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    seed_two_fetch_values(temp_path.path());

    let out = run_libra_command(&["config", "remote.origin.fetch", "NEW"], temp_path.path());
    assert_eq!(
        out.status.code(),
        Some(5),
        "ambiguous default set must exit 5, got: {:?}",
        out.status
    );
    // The original values must be untouched.
    let got = run_libra_command(
        &["config", "--get-all", "remote.origin.fetch"],
        temp_path.path(),
    );
    let stdout = String::from_utf8_lossy(&got.stdout);
    assert!(
        stdout.contains("origin/a") && stdout.contains("origin/b"),
        "values unchanged, got: {stdout:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_replace_all_collapses_to_single_value() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    seed_two_fetch_values(temp_path.path());

    let out = run_libra_command(
        &["config", "--replace-all", "remote.origin.fetch", "NEW"],
        temp_path.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "replace-all should succeed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = run_libra_command(
        &["config", "--get-all", "remote.origin.fetch", "--null"],
        temp_path.path(),
    );
    assert_eq!(
        got.stdout, b"NEW\0",
        "replace-all must leave exactly one value"
    );
}

#[tokio::test]
#[serial]
async fn test_config_set_all_subcommand_matches_replace_all() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    seed_two_fetch_values(temp_path.path());

    let out = run_libra_command(
        &["config", "set", "--all", "remote.origin.fetch", "NEW"],
        temp_path.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "set --all should succeed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = run_libra_command(
        &["config", "--get-all", "remote.origin.fetch", "--null"],
        temp_path.path(),
    );
    assert_eq!(got.stdout, b"NEW\0");
}

#[tokio::test]
#[serial]
async fn test_config_replace_all_value_filter_no_match_inserts() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "--add", "remote.origin.fetch", "main"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &[
            "config",
            "--replace-all",
            "remote.origin.fetch",
            "NEW",
            "--value",
            "^missing$",
        ],
        temp_path.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "no-match replace-all inserts and exits 0"
    );
    let got = run_libra_command(
        &["config", "--get-all", "remote.origin.fetch"],
        temp_path.path(),
    );
    let stdout = String::from_utf8_lossy(&got.stdout);
    assert!(
        stdout.contains("main\n") && stdout.contains("NEW\n"),
        "original + inserted, got: {stdout:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_get_value_filter_returns_last_match() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    for v in ["main", "dev", "main-2"] {
        run_libra_command(
            &["config", "--add", "remote.origin.fetch", v],
            temp_path.path(),
        );
    }
    let out = run_libra_command(
        &["config", "--get", "remote.origin.fetch", "--value", "^main"],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "main-2\n",
        "last matching value"
    );
}

#[tokio::test]
#[serial]
async fn test_config_get_all_value_filter_and_no_match_exit_1() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    for v in ["main", "dev"] {
        run_libra_command(
            &["config", "--add", "remote.origin.fetch", v],
            temp_path.path(),
        );
    }
    let matched = run_libra_command(
        &[
            "config",
            "--get-all",
            "remote.origin.fetch",
            "--value",
            "^main$",
        ],
        temp_path.path(),
    );
    assert_eq!(matched.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&matched.stdout), "main\n");

    let no_match = run_libra_command(
        &[
            "config",
            "--get-all",
            "remote.origin.fetch",
            "--value",
            "^missing$",
        ],
        temp_path.path(),
    );
    assert_eq!(
        no_match.status.code(),
        Some(1),
        "value read no-match exits 1"
    );
    assert!(no_match.stdout.is_empty(), "no-match stdout must be empty");
}

#[tokio::test]
#[serial]
async fn test_config_get_regexp_value_filter_pairs_only() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "remote.origin.fetch", "main"],
        temp_path.path(),
    );
    run_libra_command(
        &["config", "set", "remote.origin.url", "ssh://example"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &["config", "--get-regexp", "^remote\\.", "--value", "^main$"],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("remote.origin.fetch main\n"),
        "matching pair present, got: {stdout:?}"
    );
    assert!(
        !stdout.contains("remote.origin.url"),
        "non-matching pair excluded, got: {stdout:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_get_regexp_ignore_case_on_key_and_value() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "remote.origin.fetch", "main"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &[
            "config",
            "--get-regexp",
            "^REMOTE\\.",
            "--value",
            "^MAIN$",
            "--ignore-case",
        ],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0), "ignore-case key+value match");
    assert!(String::from_utf8_lossy(&out.stdout).contains("remote.origin.fetch main"));

    // Short form `-i` behaves the same.
    let short = run_libra_command(
        &[
            "config",
            "--get-regexp",
            "^remote\\.",
            "--value",
            "^MAIN$",
            "-i",
        ],
        temp_path.path(),
    );
    assert_eq!(short.status.code(), Some(0));
}

#[tokio::test]
#[serial]
async fn test_config_fixed_value_ignore_case_is_case_sensitive() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "--add", "remote.origin.fetch", "main"],
        temp_path.path(),
    );

    // Fixed-value literal matching is case-sensitive even with --ignore-case.
    let out = run_libra_command(
        &[
            "config",
            "--get-all",
            "remote.origin.fetch",
            "--value",
            "MAIN",
            "--fixed-value",
            "--ignore-case",
        ],
        temp_path.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "fixed-value MAIN must not match stored main"
    );
}

#[tokio::test]
#[serial]
async fn test_config_unset_value_filter_and_negation_and_fixed() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    for v in ["keep", "drop1", "drop2"] {
        run_libra_command(&["config", "--add", "branch.x.merge", v], temp_path.path());
    }
    // Negated value: remove everything that is NOT `keep` (unset-all variant).
    let out = run_libra_command(
        &[
            "config",
            "--unset-all",
            "branch.x.merge",
            "--value",
            "!^keep$",
        ],
        temp_path.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "negated unset, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = run_libra_command(
        &["config", "--get-all", "branch.x.merge", "--null"],
        temp_path.path(),
    );
    assert_eq!(got.stdout, b"keep\0", "only `keep` should remain");
}

#[tokio::test]
#[serial]
async fn test_config_unset_fixed_value_literal_dot() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "--add", "branch.x.merge", "a.b"],
        temp_path.path(),
    );
    run_libra_command(
        &["config", "--add", "branch.x.merge", "axb"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &[
            "config",
            "--unset-all",
            "branch.x.merge",
            "--value",
            "a.b",
            "--fixed-value",
        ],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let got = run_libra_command(
        &["config", "--get-all", "branch.x.merge", "--null"],
        temp_path.path(),
    );
    assert_eq!(
        got.stdout, b"axb\0",
        "fixed-value `a.b` removes only the literal, keeps axb"
    );
}

#[tokio::test]
#[serial]
async fn test_config_invalid_value_regex_exits_6_without_mutation() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "--add", "branch.x.merge", "stable"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &["config", "--unset-all", "branch.x.merge", "--value", "["],
        temp_path.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(6),
        "invalid regex exits 6, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The value must survive.
    let got = run_libra_command(
        &["config", "--get-all", "branch.x.merge", "--null"],
        temp_path.path(),
    );
    assert_eq!(
        got.stdout, b"stable\0",
        "config must be unchanged after invalid regex"
    );

    // Fixed-value treats `[` as a literal — no regex error.
    let fixed = run_libra_command(
        &[
            "config",
            "--get-all",
            "branch.x.merge",
            "--value",
            "[",
            "--fixed-value",
        ],
        temp_path.path(),
    );
    assert_eq!(
        fixed.status.code(),
        Some(1),
        "literal `[` simply does not match (exit 1)"
    );
}

#[tokio::test]
#[serial]
async fn test_config_overlong_regex_exits_6() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "remote.origin.fetch", "main"],
        temp_path.path(),
    );

    let long = "a".repeat(5000);
    let out = run_libra_command(&["config", "--get-regexp", &long], temp_path.path());
    assert_eq!(out.status.code(), Some(6), "over-long key regex exits 6");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("regex pattern is too long"),
        "stderr should mention the length cap"
    );
}

#[tokio::test]
#[serial]
async fn test_config_rejects_genuinely_invalid_keys_exit_1() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    for key in ["invalid_key", ".foo", "foo.", "a..b"] {
        let out = run_libra_command(&["config", "set", key, "x"], temp_path.path());
        assert_eq!(
            out.status.code(),
            Some(1),
            "key `{key}` should be rejected with exit 1, got: {:?}",
            out.status
        );
    }
}

#[tokio::test]
#[serial]
async fn test_config_accepts_legal_non_classic_keys() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    for key in [
        "cloud.clone_domains.example.account_id",
        "custom.api_token",
        "sec.key.123",
        "core.bigFileThreshold",
    ] {
        let out = run_libra_command(&["config", "set", key, "value"], temp_path.path());
        assert_eq!(
            out.status.code(),
            Some(0),
            "key `{key}` should be accepted, stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[tokio::test]
#[serial]
async fn test_config_sensitive_value_filter_reveal_path() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    // Auto-encrypted sensitive key with an explicit value.
    run_libra_command(
        &["config", "set", "vault.env.SECRET", "plainsecret"],
        temp_path.path(),
    );

    // Without --reveal, the filter runs over ciphertext; the plaintext pattern
    // does not match, and nothing is leaked.
    let no_reveal = run_libra_command(
        &[
            "config",
            "--get",
            "vault.env.SECRET",
            "--value",
            "^plainsecret$",
        ],
        temp_path.path(),
    );
    assert_eq!(
        no_reveal.status.code(),
        Some(1),
        "ciphertext filter should not match plaintext pattern"
    );
    assert!(
        !String::from_utf8_lossy(&no_reveal.stdout).contains("plainsecret"),
        "secret must not leak without --reveal"
    );

    // With --reveal, the value is decrypted, the filter matches, and the
    // plaintext is returned — proving --reveal is wired into the filter path.
    let revealed = run_libra_command(
        &[
            "config",
            "--get",
            "vault.env.SECRET",
            "--reveal",
            "--value",
            "^plainsecret$",
        ],
        temp_path.path(),
    );
    assert_eq!(
        revealed.status.code(),
        Some(0),
        "reveal+filter should match, stderr: {}",
        String::from_utf8_lossy(&revealed.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&revealed.stdout), "plainsecret\n");
}

// ─────────────────────────────────────────────────────────────────────────────
// Wave 4 — section operations (rename-section / remove-section)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_config_rename_section_moves_all_dotted_keys() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "remote.origin.url", "ssh://x"],
        temp_path.path(),
    );
    run_libra_command(
        &["config", "set", "remote.origin.fetch", "+a"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &[
            "config",
            "rename-section",
            "remote.origin",
            "remote.upstream",
        ],
        temp_path.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "rename-section should succeed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let names = run_libra_command(&["config", "--list", "--name-only"], temp_path.path());
    let stdout = String::from_utf8_lossy(&names.stdout);
    assert!(stdout.contains("remote.upstream.url") && stdout.contains("remote.upstream.fetch"));
    assert!(
        !stdout.contains("remote.origin."),
        "no remote.origin.* should remain, got: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_rename_section_flag_form_matches_subcommand() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "remote.origin.url", "ssh://x"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &[
            "config",
            "--rename-section",
            "remote.origin",
            "remote.upstream",
        ],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let got = run_libra_command(
        &["config", "--get", "remote.upstream.url"],
        temp_path.path(),
    );
    assert_eq!(String::from_utf8_lossy(&got.stdout), "ssh://x\n");
}

#[tokio::test]
#[serial]
async fn test_config_rename_section_conflict_exit_5_no_partial_write() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "remote.origin.url", "src"],
        temp_path.path(),
    );
    run_libra_command(
        &["config", "set", "remote.upstream.url", "dst"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &[
            "config",
            "rename-section",
            "remote.origin",
            "remote.upstream",
        ],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(5), "rename conflict must exit 5");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("target section 'remote.upstream' already exists"),
        "stderr: {stderr}"
    );
    // Both sections must be unchanged.
    let origin = run_libra_command(&["config", "--get", "remote.origin.url"], temp_path.path());
    assert_eq!(String::from_utf8_lossy(&origin.stdout), "src\n");
}

#[tokio::test]
#[serial]
async fn test_config_remove_section_deletes_all_keys() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "branch.main.remote", "origin"],
        temp_path.path(),
    );
    run_libra_command(
        &["config", "set", "branch.main.merge", "refs/heads/main"],
        temp_path.path(),
    );
    run_libra_command(
        &["config", "set", "branch.dev.remote", "origin"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &["config", "remove-section", "branch.main"],
        temp_path.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "remove-section should succeed, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let names = run_libra_command(&["config", "--list", "--name-only"], temp_path.path());
    let stdout = String::from_utf8_lossy(&names.stdout);
    assert!(!stdout.contains("branch.main."), "branch.main.* removed");
    assert!(
        stdout.contains("branch.dev.remote"),
        "sibling section survives"
    );
}

#[tokio::test]
#[serial]
async fn test_config_remove_missing_section_exit_5() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let out = run_libra_command(&["config", "remove-section", "no.such"], temp_path.path());
    assert_eq!(
        out.status.code(),
        Some(5),
        "missing section removal exits 5"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("section 'no.such' does not exist"),
        "stderr: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_section_ops_scope_isolation() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let global_db_dir = tempdir().unwrap();
    let _scoped = ScopedConfigPathGuard::new(&global_db_dir.path().join("global_section.db"));

    // Same section name in both scopes.
    run_libra_command(
        &["config", "set", "remote.origin.url", "local-url"],
        temp_path.path(),
    );
    run_libra_command(
        &[
            "config",
            "--global",
            "set",
            "remote.origin.url",
            "global-url",
        ],
        temp_path.path(),
    );

    // Local rename must not touch the global section.
    run_libra_command(
        &[
            "config",
            "rename-section",
            "remote.origin",
            "remote.renamed",
        ],
        temp_path.path(),
    );

    let global = run_libra_command(
        &["config", "--global", "--get", "remote.origin.url"],
        temp_path.path(),
    );
    assert_eq!(
        String::from_utf8_lossy(&global.stdout),
        "global-url\n",
        "global section unchanged"
    );
}

#[tokio::test]
#[serial]
async fn test_config_generic_rename_has_no_remote_side_effects() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "remote.origin.url", "ssh://x"],
        temp_path.path(),
    );
    run_libra_command(
        &["config", "set", "branch.main.remote", "origin"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &[
            "config",
            "rename-section",
            "remote.origin",
            "remote.upstream",
        ],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    // branch.main.remote must still point at "origin" (no remote-cascade rewrite).
    let br = run_libra_command(&["config", "--get", "branch.main.remote"], temp_path.path());
    assert_eq!(
        String::from_utf8_lossy(&br.stdout),
        "origin\n",
        "branch.*.remote untouched"
    );
}

#[tokio::test]
#[serial]
async fn test_config_vault_section_rename_rejected_129() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "vault.env.TEST_SECRET", "shh"],
        temp_path.path(),
    );

    for argv in [
        vec!["config", "remove-section", "vault.env"],
        vec!["config", "rename-section", "vault.env", "vault.elsewhere"],
        vec!["config", "rename-section", "remote.origin", "vault.sneaky"],
    ] {
        let out = run_libra_command(&argv, temp_path.path());
        assert_eq!(
            out.status.code(),
            Some(129),
            "vault section op must exit 129: {argv:?}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("vault sections must be managed by dedicated vault/config commands"),
            "argv {argv:?} stderr: {stderr}"
        );
    }
    // The secret must still exist (redacted).
    let got = run_libra_command(
        &["config", "--get", "vault.env.TEST_SECRET"],
        temp_path.path(),
    );
    assert_eq!(got.status.code(), Some(0), "secret must still be present");
}

#[tokio::test]
#[serial]
async fn test_config_invalid_section_name_rejected_no_change() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(&["config", "set", "keep.this.x", "v"], temp_path.path());

    for bad in ["a..b", ".lead", "trail."] {
        let out = run_libra_command(&["config", "remove-section", bad], temp_path.path());
        assert_eq!(
            out.status.code(),
            Some(1),
            "invalid section `{bad}` should exit 1"
        );
    }
    let got = run_libra_command(&["config", "--get", "keep.this.x"], temp_path.path());
    assert_eq!(
        String::from_utf8_lossy(&got.stdout),
        "v\n",
        "unrelated key unchanged"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Wave 5 — typed value normalization (--type=bool|int|path)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_config_type_bool_canonicalizes_get() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "feature.enabled", "yes"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &["config", "--type=bool", "--get", "feature.enabled"],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "true\n");
}

#[tokio::test]
#[serial]
async fn test_config_type_int_suffix_get() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "core.bigfilethreshold", "1k"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &["config", "--type=int", "--get", "core.bigfilethreshold"],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1024\n");
}

#[tokio::test]
#[serial]
async fn test_config_type_int_overflow_exits_2_no_panic() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "core.size", "9223372036854775807g"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &["config", "--type=int", "--get", "core.size"],
        temp_path.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "int overflow must exit 2, not panic"
    );
}

#[tokio::test]
#[serial]
async fn test_config_type_int_set_invalid_rejected_no_store() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let set = run_libra_command(
        &["config", "--type=int", "set", "core.size", "nope"],
        temp_path.path(),
    );
    assert_eq!(set.status.code(), Some(2), "invalid int set exits 2");
    let got = run_libra_command(&["config", "--get", "core.size"], temp_path.path());
    assert_ne!(
        got.status.code(),
        Some(0),
        "core.size must not have been stored"
    );
}

#[tokio::test]
#[serial]
async fn test_config_type_path_expands_tilde() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "core.excludesfile", "~/ignore"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &["config", "--type=path", "--get", "core.excludesfile"],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let home = temp_path.path().join(".libra-test-home");
    let expected = format!("{}\n", home.join("ignore").to_string_lossy());
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[tokio::test]
#[serial]
async fn test_config_type_path_rejects_tilde_user() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "core.excludesfile", "~alice/ignore"],
        temp_path.path(),
    );

    let out = run_libra_command(
        &["config", "--type=path", "--get", "core.excludesfile"],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("~user path expansion is not supported by Libra config")
    );
    assert!(out.stdout.is_empty());
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_config_type_path_unset_home_exits_1() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(
        &["config", "set", "core.excludesfile", "~/ignore"],
        temp_path.path(),
    );

    // Override HOME to empty: `~` cannot be expanded.
    let out = run_libra_command_with_stdin_and_env(
        &["config", "--type=path", "--get", "core.excludesfile"],
        temp_path.path(),
        "",
        &[("HOME", "")],
    );
    assert_eq!(out.status.code(), Some(1), "empty HOME must exit 1");
}

#[tokio::test]
#[serial]
async fn test_config_type_encrypted_redacted_without_reveal_then_revealed() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    // Auto-encrypted sensitive key holding a bool-like value.
    run_libra_command(
        &["config", "set", "vault.env.FLAG", "yes"],
        temp_path.path(),
    );

    let redacted = run_libra_command(
        &["config", "--type=bool", "--get", "vault.env.FLAG"],
        temp_path.path(),
    );
    assert_eq!(redacted.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&redacted.stdout),
        "<REDACTED>\n",
        "must not parse ciphertext"
    );

    let revealed = run_libra_command(
        &[
            "config",
            "--type=bool",
            "--get",
            "vault.env.FLAG",
            "--reveal",
        ],
        temp_path.path(),
    );
    assert_eq!(
        revealed.status.code(),
        Some(0),
        "reveal+type should canonicalize, stderr: {}",
        String::from_utf8_lossy(&revealed.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&revealed.stdout), "true\n");
}

#[tokio::test]
#[serial]
async fn test_config_type_deferred_and_combo_rejections_129() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    run_libra_command(&["config", "set", "user.name", "Ada"], temp_path.path());

    for argv in [
        vec!["config", "--type=color", "--get", "color.ui"],
        vec!["config", "--type=bool", "--list"],
        vec!["config", "--no-type", "--get", "user.name"],
    ] {
        let out = run_libra_command(&argv, temp_path.path());
        assert_eq!(out.status.code(), Some(129), "{argv:?} should exit 129");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Wave 6 — explicit rejection of unsupported selectors + global DB 0600
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_config_unsupported_selectors_rejected_129() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let cases: &[(&[&str], &str)] = &[
        (&["config", "--worktree", "--get", "user.name"], "worktree"),
        (&["config", "--blob", "HEAD:.gitconfig", "--list"], "blob"),
        (&["config", "--includes", "--list"], "include"),
        (&["config", "--no-includes", "--list"], "include"),
        (&["config", "--get-color", "color.ui"], "color"),
        (&["config", "--get-colorbool", "color.ui"], "color"),
        (&["config", "--no-value", "--get", "user.name"], "no-value"),
        (
            &["config", "--show-names", "--get", "user.name"],
            "show-names",
        ),
        (&["config", "--get-urlmatch", "http", "https://x"], "url"),
    ];
    for (argv, needle) in cases {
        let out = run_libra_command(argv, temp_path.path());
        assert_eq!(out.status.code(), Some(129), "{argv:?} should exit 129");
        let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
        assert!(
            stderr.contains(needle),
            "stderr for {argv:?} should mention `{needle}`: {stderr}"
        );
    }
}

#[tokio::test]
#[serial]
async fn test_config_file_selector_rejected_suggests_import() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let out = run_libra_command(
        &["config", "--file", "/tmp/example.gitconfig", "--list"],
        temp_path.path(),
    );
    assert_eq!(out.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--file is not supported"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("libra config --import"),
        "should suggest --import: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_config_supported_global_still_works() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let set = run_libra_command(
        &["config", "--global", "user.name", "Ada"],
        temp_path.path(),
    );
    assert_eq!(set.status.code(), Some(0));
    let get = run_libra_command(
        &["config", "--global", "--get", "user.name"],
        temp_path.path(),
    );
    assert_eq!(get.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&get.stdout), "Ada\n");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_config_global_db_created_with_0600() {
    use std::os::unix::fs::PermissionsExt;
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;

    let set = run_libra_command(
        &["config", "--global", "user.name", "Ada"],
        temp_path.path(),
    );
    assert_eq!(set.status.code(), Some(0));

    // base_libra_command points LIBRA_CONFIG_GLOBAL_DB at this isolated path.
    let global_db = temp_path
        .path()
        .join(".libra-test-home")
        .join(".libra")
        .join("config.db");
    assert!(
        global_db.exists(),
        "global DB should have been created at {global_db:?}"
    );
    let mode = std::fs::metadata(&global_db).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "global config DB must be 0600, got {:o}",
        mode & 0o777
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Wave 7 — documentation contract (closed flag list ↔ docs ↔ `--help`)
// ─────────────────────────────────────────────────────────────────────────────

/// Closed list of decision-ledger flags that must be both documented and
/// surfaced by `config --help`. A closed list avoids false failures when prose
/// mentions a flag that is intentionally not in EXAMPLES.
const REQUIRED_LEDGER_FLAGS: &[&str] =
    &["--null", "--show-origin", "--show-scope", "--replace-all"];

#[test]
fn config_doc_ledger_flags_are_documented_and_helpable() {
    let doc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/commands/config.md"
    ))
    .expect("read docs/commands/config.md");
    let home = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_libra"))
        .args(["config", "--help"])
        .env("HOME", home.path())
        .env("LANG", "C")
        .output()
        .expect("run config --help");
    assert!(out.status.success(), "config --help must succeed");
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in REQUIRED_LEDGER_FLAGS {
        assert!(
            doc.contains(flag),
            "docs/commands/config.md must document {flag}"
        );
        assert!(
            help.contains(flag),
            "`libra config --help` must expose {flag} (via EXAMPLES) once implemented"
        );
    }
}
